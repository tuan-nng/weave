# api/ — HTTP Handlers

Axum route handlers returning JSON with `{ "data": ... }` or `{ "error": ... }` envelope. Router assembly in `mod.rs`.

## Files

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 4KB | `router(state)` — assembles all sub-routers under `/api/...` + SPA fallback |
| `kanban.rs` | 78KB | Board/column/task CRUD + board SSE stream — largest handler file |
| `sessions.rs` | 29KB | Session CRUD, send_prompt, cancel, SSE stream, resume |
| `codebases.rs` | 23KB | Codebase CRUD endpoints |
| `providers.rs` | 20KB | Provider CRUD + list_models |
| `workspaces.rs` | 13KB | Workspace CRUD |
| `traces.rs` | 9KB | Trace/journey/file-changes read endpoints |
| `static_assets.rs` | 7KB | Embedded frontend serving via `ServeDir` fallback |
| `specialists.rs` | 5KB | List specialists endpoint |
| `health.rs` | 767B | `GET /api/health` — returns uptime + db status |
| `responses.rs` | 170B | `ApiResponse<T>` type — `{ success, data?, error? }` |

## Key Patterns

- Router structure: `/api/workspaces/:wid/<resource>` — workspace scoping via path param
- All handlers extract `AppState` and `Query<Pagination>` from Axum state/extensions
- SSE streaming: `session_stream`, `board_stream` — use `SseManager` for fan-out
- `ApiResponse<T>` wrapper used for all JSON responses
- Module namespace mirrors `store/` — `api::sessions` wraps `store::sessions`

## Connections

- **Calls:** `store/*` for data, `service::SessionService` for orchestration, `agent::ProviderRegistry` for agent access
- **Called by:** Nothing (leaf handlers — called by Axum router)
- **Shared types:** `ApiResponse` in `responses.rs`, `ServerStartTime` extractor in `mod.rs`
