# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-05-31
- **Latest commit:** (pending — feat-010)
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 128 tests pass (8 new for feat-010 + 120 existing)
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

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-011: SpecialistLoader (depends on feat-009 — passing)
2. Continue Phase 2 (Agent Tools & Observability)

## Notes for Next Session

- feat-010 created: `src/sse/mod.rs`
- `SseManager` holds per-entity `broadcast::channel(256)` + `EventBuffer` (ring buffer, 100 events) + `AtomicU64` counter
- Event IDs start at 1 (not 0) — `get_after(0)` returns all buffered events
- `SseWireEvent` enum wraps `StreamEvent` variants + `Connected`/`Gap` SSE-protocol events
- `stream_event_to_wire()` converts agent events to wire format
- `session_stream` handler uses `stream::unfold` with `SseState` state machine (Initial → Gap/Buffered/Live → Done)
- Deduplication: subscribe first, read buffer, track `max_buffered_id`, skip receiver events with ID ≤ max_buffered_id
- `make_sse_event(id, event_type, data)` sets `.id()` on every event for `Last-Event-ID` support
- Session existence check: nonexistent sessions get an error event + stream close (not 404 — SSE can't return HTTP errors)
- `SessionService::send_prompt` now accepts `&Arc<sse::SseManager>` and broadcasts every `StreamEvent` via `sse::stream_event_to_wire()`
- Cancellation broadcasts `Done { stop_reason: Cancelled }` before updating session status
- Heartbeat: `Sse::new(stream).keep_alive(KeepAlive::default())` sends SSE comment lines every 15s
- `AppState` now has `sse_manager: Arc<sse::SseManager>` field
- `ActiveSessions` + `SseManager` are separate types — `ActiveSessions` tracks cancellation tokens, `SseManager` handles broadcast/buffer
- TOCTOU race in `broadcast()` fixed: single write lock for channel creation
- Phase 1 (Core Foundation) is now complete — all 10 features passing

## Out-of-Scope Items Noticed

(none yet)
