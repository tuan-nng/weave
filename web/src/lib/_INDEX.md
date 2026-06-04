# web/src/lib/ — Frontend Infrastructure

Shared types, API client, query key factory, and route definitions. No React dependencies — pure TypeScript modules.

## Files

| File            | Size | Contains                                                                                                                                                                                                                                                                                                                                                                                                                |
| --------------- | ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `types.ts`      | 12KB | All domain types matching Rust backend structs: `Workspace`, `Session`, `Message`, `Provider`, `Board`/`BoardDetail`, `Column`, `Task`, `SpecialistInfo`, `TraceRow`, `FileChangeSummary`, `Codebase`/`CodebaseDetail`, `HealthResponse`, `ModelInfo`. Request types: `Create*Request`, `Update*Request`. Response wrappers: `ApiResponse<T>`, `PaginatedResponse<T>`, `PaginationParams`. Enum types: `SessionStatus`. |
| `api.ts`        | 9KB  | API client — typed `fetch` wrappers for every endpoint. `apiClient.get/post/patch/delete<T>(url, body?)`. Exports named functions: `fetchWorkspaces`, `createWorkspace`, `fetchSessions`, `createSession`, `sendPrompt`, `fetchBoard`, etc.                                                                                                                                                                             |
| `query-keys.ts` | 2KB  | TanStack Query key factory — hierarchical keys: `queryKeys.health()`, `queryKeys.workspaces.list(params)`, `queryKeys.sessions.detail(id)`, `queryKeys.boards.detail(id)`, etc.                                                                                                                                                                                                                                         |
| `routes.ts`     | 1KB  | Route path constants: `ROUTES.home`, `ROUTES.workspace(id)`, `ROUTES.session(wid, sid)`, `ROUTES.board(wid, bid)`, `ROUTES.codebase(wid, cid)`, `ROUTES.settings(wid)`                                                                                                                                                                                                                                                  |

## Key Patterns

- `api.ts` functions throw on non-2xx responses — error handling at hook/call-site level
- `query-keys.ts` uses nested const assertions for type-safe cache invalidation
- `types.ts` mirrors backend Rust structs — kept in sync manually
- `routes.ts` centralizes path construction — no hardcoded URLs in components

## Connections

- **Used by:** `hooks/` (data layer), `app/pages/*` (components)
- **Depends on:** Nothing in the frontend — this is the foundation layer
