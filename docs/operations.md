# Operations

Build, deployment, security, and operational concerns.

## Binary Embedding

The Rust binary embeds the built frontend assets via `build.rs`:

```rust
// build.rs
fn main() {
    // Build frontend
    let output = Command::new("npm")
        .args(["run", "build"])
        .current_dir("../web")
        .output()
        .expect("Failed to build frontend");

    if !output.status.success() {
        panic!("Frontend build failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    // Tell cargo to re-run if frontend files change
    println!("cargo:rerun-if-changed=../web/src");
    println!("cargo:rerun-if-changed=../web/package.json");
}
```

At runtime, assets served via `tower-http::services::ServeDir` with SPA fallback. Production embedding uses `include_dir!` for a single-file binary.

## Dependencies

### Rust
| Crate | Purpose |
|-------|---------|
| `axum` | HTTP framework |
| `tokio` | Async runtime |
| `rusqlite` (bundled) | SQLite driver |
| `serde` / `serde_json` | Serialization |
| `uuid` | ID generation |
| `chrono` | Timestamps |
| `tracing` / `tracing-subscriber` | Logging |
| `reqwest` | HTTP client for provider APIs |
| `tokio-stream` | SSE streaming utilities |
| `tower` / `tower-http` | Middleware, static file serving |
| `anyhow` / `thiserror` | Error handling |
| `clap` | CLI argument parsing |
| `serde_yaml` | Specialist frontmatter parsing |

### Frontend
| Package | Purpose |
|---------|---------|
| `react` / `react-dom` | UI framework |
| `react-router` | Client-side routing |
| `@tanstack/react-query` | Data fetching + caching |
| `tailwindcss` | Styling |
| `@dnd-kit/core` | Drag-and-drop for kanban |
| `marked` / `react-markdown` | Markdown rendering |
| `zod` | Client-side validation |

## Testing Strategy

| Layer | Approach |
|-------|----------|
| **Store (unit)** | In-memory SQLite (`:memory:`) per test, verify CRUD |
| **Domain services (unit)** | Mock stores, verify business logic and state transitions |
| **Agent (unit)** | Mock HTTP responses, verify stream event parsing |
| **API routes (integration)** | `axum::test` helpers, verify request/response contracts |
| **SSE (integration)** | Verify event ordering, reconnection, heartbeat |
| **Kanban (integration)** | Verify column transition triggers session creation |
| **E2E** | Session lifecycle + kanban flow end-to-end |

## Security Considerations

### API Key Storage
Provider API keys stored in SQLite `config_json` column. Plaintext for v1; future: encrypt at rest. API keys never returned in API responses — `GET /api/providers` strips `api_key`.

### Input Validation
- Workspace names: 1-100 characters, no control characters
- Session prompts: max 100KB
- Task titles: 1-500 characters
- File paths: must be absolute, no `..` traversal

### CORS
Same-origin only — frontend served from the same binary.

### Threat Model (v1)
**Assumptions:** Single user on local machine, binds to `127.0.0.1` by default, no authentication.

| Risk | Severity | Mitigation |
|------|----------|------------|
| Binding to `0.0.0.0` exposes API | HIGH | Require `--allow-remote` flag; log WARN on non-localhost |
| API key in plaintext SQLite | MEDIUM | File permissions `600` on `weave.db`; future encrypt-at-rest |
| Agent executes arbitrary shell commands | MEDIUM | No sandboxing in v1; document this clearly |
| No CSRF protection | LOW | Same-origin SPA = no CSRF vector |
| No rate limiting on Weave API | LOW | Single-user assumption |

### Rate Limiting
Provider API rate limits handled with exponential backoff. Weave API itself does not rate-limit for v1 (single-user assumption).

## Logging and Observability

Uses `tracing` crate with JSON output for production:

```rust
tracing_subscriber::fmt()
    .with_env_filter(
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("weave=info,tower_http=info"))
    )
    .json()
    .init();
```

| Level | What |
|-------|------|
| `ERROR` | Provider failures, database errors, unrecoverable state |
| `WARN` | Skipped specialist files, slow queries (>100ms), SSE lag |
| `INFO` | Server startup/shutdown, session created/completed, provider added |
| `DEBUG` | Tool executions, message saves, SSE events |
| `TRACE` | Raw HTTP requests/responses, SQL queries |

Set `RUST_LOG=weave=debug` for development. Default is `info`.

### Health Check
`GET /api/health` returns status, version, uptime, provider health counts, active sessions, and database metrics (size, WAL checkpoint pending).

## Database Operations

### Backup (WAL-aware)
```
sqlite3 weave.db "PRAGMA wal_checkpoint(TRUNCATE);"
cp weave.db weave.db.backup
```

Future: `POST /api/admin/backup` endpoint (Phase 6).

### Migration to New Machine
1. Stop Weave server
2. Copy `weave.db` (and `.db-wal`, `.db-shm` if they exist) to new machine
3. Start Weave — runs migrations automatically

**Caveat**: Provider `config_json` may contain host-specific config; providers may need reconfiguration after migration.

## Performance Notes
- WAL mode allows concurrent reads during writes
- Indexes on foreign keys and frequently queried columns
- SSE: `broadcast::channel(256)` per entity, 100-event buffer cap, 15s heartbeat
- Frontend: TanStack Query caching, SSE incremental updates, code splitting per route
