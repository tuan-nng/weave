# src/ — Application Entry & Foundations

The crate root. Binary entrypoint, database setup, error types, and configuration.

## Files

| File | Size | Contains |
|------|------|----------|
| `main.rs` | 7KB | `main()` — parses CLI args, opens DB, runs migrations, creates `AppState`, spawns SSE manager + trace collector + provider registry, mounts Axum router, binds TCP listener, graceful shutdown via Ctrl+C/SIGTERM |
| `db.rs` | 9KB | `Db` struct (SQLite connection pool via `r2d2`), `open(path)` (WAL mode, foreign keys, busy timeout), `run_migrations()` (SQL files from `resources/migrations/`), `with_transaction()`, `map_insert_error()` (UNIQUE → Conflict) |
| `error.rs` | 9KB | `AppError` enum (NotFound, Conflict, Validation, Internal, AuthFailed, Timeout, ExecuteReturnedResults) + `into_response()` for Axum, `ProviderError` (RateLimited, AuthFailed, Overloaded, ServerError) |
| `config.rs` | 703B | `Config` struct — `host`, `port`, `db_path`, `allow_remote`. Parsed from CLI args via `clap`. |

## Key Patterns

- `AppState` holds: `Db`, `SseManager`, `TraceCollector`, `ProviderRegistry`, `SpecialistRegistry`, `ToolRegistry`, `SessionService`, `Config`
- `main()` is the only place `Arc::new()` and `.clone()` wire up shared state
- Graceful shutdown: `tokio::select!` between `axum::serve()` and `shutdown_signal()`
- Migrations are embedded at compile time via `include_str!()` in `db.rs`

## Connections

- **Creates:** All subsystems wired in `main.rs`
- **Depends on:** All other modules — this is the composition root
