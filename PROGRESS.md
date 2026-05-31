# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-05-31
- **Latest commit:** (pending — feat-004)
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 20 tests pass (9 store + 5 api + 6 existing)
- **Lint status:** green — clippy clean, fmt clean

## Completed Since Project Start

- [x] System design documentation (`docs/SYSTEM_DESIGN.md`, `docs/ARCHITECTURE.md`)
- [x] Implementation plan (`docs/PLAN.md`)
- [x] Workspace `Cargo.toml` created (members: `crates/weave-server`)
- [x] **feat-001**: Binary skeleton with CLI, tracing, health check, graceful shutdown
- [x] **feat-002**: SQLite database with WAL mode and migrations (11 tables, user_version tracking)
- [x] **feat-003**: Shared error types (AppError, ProviderError) with thiserror, IntoResponse, JSON envelope
- [x] **feat-004**: Workspace CRUD (WorkspaceStore, REST API, default workspace seed, cursor pagination)

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-005: CodingAgent trait (depends on feat-001 — passing)
2. Start feat-006: AnthropicAgent (depends on feat-005)
3. Continue Phase 1 (Core Foundation): feat-007 through feat-010

## Notes for Next Session

- feat-004 created: `src/store/mod.rs`, `src/store/workspaces.rs`, `src/api/workspaces.rs`, `src/api/responses.rs`, `src/migrations/003_workspace_unique_name.sql`
- `WorkspaceStore` is a stateless struct with static methods taking `&Db`
- Cursor pagination uses `WHERE id > ?cursor ORDER BY id ASC LIMIT ?N` with last row's id as cursor
- `DataResponse<T>` in `api/responses.rs` wraps all success responses in `{"data": ...}` envelope
- Default workspace "default" is seeded at startup via `WorkspaceStore::ensure_default` in `main.rs`
- Default workspace cannot be renamed or deleted (checked in API handlers by name == "default")
- UNIQUE index on `workspaces.name` added via migration 003
- `WorkspaceStore::delete` takes `name` parameter for logging (avoids redundant re-fetch)
- `validate_name` uses `name.chars().count()` for Unicode-correct character counting
- `tower` added as dev-dependency for integration test `oneshot` calls
- `cargo fmt` was auto-applied; all formatting is clean
- feat-005 (CodingAgent trait) can start immediately — depends only on feat-001
- feat-007 (ProviderStore) depends on feat-004 + feat-006 — blocked until both pass

## Out-of-Scope Items Noticed

(none yet)
