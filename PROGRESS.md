# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-05-31
- **Latest commit:** (pending — feat-003)
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 5 tests pass (test_db_init, test_migrations_idempotent, test_error_response_format, test_error_status_codes, test_from_conversions)
- **Lint status:** green — clippy clean, fmt clean

## Completed Since Project Start

- [x] System design documentation (`docs/SYSTEM_DESIGN.md`, `docs/ARCHITECTURE.md`)
- [x] Implementation plan (`docs/PLAN.md`)
- [x] Workspace `Cargo.toml` created (members: `crates/weave-server`)
- [x] **feat-001**: Binary skeleton with CLI, tracing, health check, graceful shutdown
- [x] **feat-002**: SQLite database with WAL mode and migrations (11 tables, user_version tracking)
- [x] **feat-003**: Shared error types (AppError, ProviderError) with thiserror, IntoResponse, JSON envelope

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-004: Workspace CRUD (depends on feat-002 + feat-003 — both passing)
2. Continue Phase 1 (Core Foundation): feat-005 through feat-010
3. Verify each feature with its verification command before moving on

## Notes for Next Session

- feat-001 created: `crates/weave-server/Cargo.toml`, `src/main.rs`, `src/config.rs`, `src/api/mod.rs`, `src/api/health.rs`
- Binary accepts `--host`, `--port`, `--db-path`, `--allow-remote` via clap
- Tracing uses `RUST_LOG` env filter, defaults to `info`
- Health endpoint: `GET /api/health` returns `{status, version, uptime_seconds}`
- Graceful shutdown on SIGTERM/SIGINT via `tokio::signal`
- feat-002 created: `src/db.rs`, `src/migrations/001_init.sql`, `src/migrations/002_kanban.sql`
- `Db` wrapper type encapsulates `Mutex<Connection>`, exposes `conn()` accessor
- `AppState { db: Arc<Db> }` is the shared state injected into Axum handlers via Extension
- Migrations use `user_version` pragma for version tracking, `include_str!` for embedding
- rusqlite 0.35 with `bundled` feature (0.40 requires nightly Rust for libsqlite3-sys)
- feat-003 created: `src/error.rs` — `AppError` enum (NotFound, Validation, Provider, Database, Internal), `ProviderError` enum (AuthFailed, RateLimited, ModelNotFound, Unreachable, StreamInterrupted)
- `AppError` implements `IntoResponse` → maps to HTTP status codes + `{"error": {"code": "...", "message": "..."}}` JSON envelope
- `Database` and `Internal` variants log real error via `tracing::error!` but return sanitized "Internal server error" to client
- `#[from]` derives on ProviderError, rusqlite::Error, anyhow::Error for automatic conversion
- `mod error;` added to main.rs alongside existing modules
- feat-004 depends on feat-002 + feat-003 — both now passing, can start immediately
- feat-005 depends on feat-001 — can start in parallel with feat-004

## Out-of-Scope Items Noticed

(none yet)
