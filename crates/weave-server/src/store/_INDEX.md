# store/ — SQLite Data Access

Raw `rusqlite` data access layer. All queries are parameterized, workspace-scoped, and use explicit row mapping. No ORM.

## Files

| File | Size | Contains |
|------|------|----------|
| `mod.rs` | 210B | Module declarations — re-exports all submodules |
| `tasks.rs` | 41KB | `TaskStore` + `Task`/`UpdateTask`/`UpdateTaskFields` — CRUD, list with filters, move to column, status transitions |
| `sessions.rs` | 28KB | `SessionStore` + `MessageStore` + `Session`/`Message` — lifecycle, pagination, message ordering, resume parent chain |
| `columns.rs` | 26KB | `ColumnStore` + `Column` — CRUD, position management, rebalancing, auto-trigger validation |
| `notes.rs` | 21KB | `NoteStore` + `Note` — CRUD, append, set_content, type filtering |
| `artifacts.rs` | 20KB | `ArtifactStore` + `Artifact` — CRUD, list by task, type set queries |
| `codebases.rs` | 20KB | `CodebaseStore` + `Codebase`/`CodebaseDetail`/`GitStatus` — CRUD, CWD-prefix matching, git info |
| `boards.rs` | 19KB | `BoardStore` + `Board`/`BoardDetail`/`NewColumnSpec` — CRUD with template columns, cascade delete |
| `traces.rs` | 17KB | `TraceStore` + `TraceEvent`/`TraceEventKind`/`FileAction`/`TraceRow` — insert batches, journey queries, file changes |
| `workspaces.rs` | 12KB | `WorkspaceStore` + `Workspace`/`WorkspacePage` — CRUD, default workspace seeding |
| `providers.rs` | 8KB | `ProviderStore` + `Provider` — CRUD, api_key stripping in responses |
| `kanban_test_helpers.rs` | 7KB | Test helpers — shared seed functions for kanban tests |

## Key Patterns

- Every `*Store` struct wraps `Arc<Db>` and takes `&self` methods
- All list/get/create/update/delete methods require `workspace_id: &str`
- `map_insert_error` (from `db.rs`) converts UNIQUE constraint violations to `AppError::Conflict`
- Test helpers use `test_db()` for in-memory SQLite with WAL mode
- Position management: floats in `columns`/`tasks` tables with rebalancing when too close

## Connections

- **Used by:** `api/*` handlers, `service/*` business logic, `tools/*` executors
- **Depends on:** `db.rs` (connection pool, migrations), `error.rs` (AppError)
