# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-05-31
- **Latest commit:** (pending — feat-009)
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 120 tests pass (17 new for feat-009 + 103 existing)
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

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-010: SSE infrastructure (depends on feat-009 — now passing)
2. Continue Phase 1 (Core Foundation): feat-010 is the last Phase 1 feature

## Notes for Next Session

- feat-009 created: `src/service/mod.rs`, `src/service/sessions.rs`
- `ActiveSessions` is a `Mutex<HashMap<String, CancellationToken>>` wrapper with atomic `try_insert` (TOCTOU-safe)
- `SessionService` is a unit struct with `send_prompt` (async) and `cancel_session` (sync)
- `send_prompt` returns user message ID immediately, spawns async task for streaming
- Cancel uses `tokio::sync::CancellationToken` with `tokio::select!` for instant cancellation
- `SessionGuard` drop pattern ensures `active_sessions.remove()` runs even on panic
- Content stored as raw text (no JSON encoding) — consistent between user and assistant messages
- `build_message_history` converts store messages to agent format as `Content::Text`
- `resolve_model` chain: session.model → provider config default_model → hardcoded fallback
- `abort_with_error` helper deduplicates error-handling blocks in the spawned task
- `load_all_messages` has MAX_HISTORY_MESSAGES=1000 cap
- API routes: `POST /api/sessions/:sid/prompt`, `POST /api/sessions/:sid/cancel`
- `send_prompt` returns 201 CREATED (consistent with other POST endpoints)
- `TERMINAL` const in `store/sessions.rs` made `pub(crate)` for service access
- `tokio-stream` added as dev-dependency for mock agent test
- `tokio-util` added as dependency for `CancellationToken`
- feat-010 (SSE infrastructure) depends on feat-009 — now passing, can start immediately

## Out-of-Scope Items Noticed

(none yet)
