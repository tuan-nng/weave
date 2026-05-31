# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-05-31
- **Latest commit:** (pending — feat-006)
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 58 tests pass (28 anthropic + 9 agent + 9 store + 5 api + 6 existing + 1 streaming)
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

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-007: ProviderStore (depends on feat-004 + feat-006 — both passing)
2. Continue Phase 1 (Core Foundation): feat-008 through feat-010

## Notes for Next Session

- feat-006 created: `src/agent/anthropic/{mod.rs, types.rs, streaming.rs}` — AnthropicAgent implementing CodingAgent
- `AnthropicAgent` struct holds `reqwest::Client`, `base_url`, `api_key`, `default_model`
- SSE parser in `streaming.rs` is a manual state machine (no external SSE crate), handles `\r\n` line endings
- `EventConverter` tracks tool_use IDs by content block index for delta routing
- Retry logic checks status codes directly (`429 | 500 | 529`), not error types — avoids retrying 400/401/404
- `ReceiverStream` wrapper in mod.rs implements `Stream` via `mpsc::Receiver::poll_recv` (avoids tokio-stream dep)
- Dependencies added: `reqwest = { version = "0.12", features = ["json", "stream"] }`, `bytes = "1"`, `futures-util = "0.3"`
- `list_models()` returns empty vec — DB-driven in feat-007
- `health_check()` sends minimal request with `max_tokens: 1` to verify credentials
- feat-007 (ProviderStore) depends on feat-004 + feat-006 — both now passing, can start immediately

## Out-of-Scope Items Noticed

(none yet)
