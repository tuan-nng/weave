# web/src/app/pages/ — React Page Components

Route-level page components + sub-components. One file per route, plus feature-specific subdirectories.

## Route Pages (top-level)

| File            | Contains                                                                     |
| --------------- | ---------------------------------------------------------------------------- |
| `home.tsx`      | `/` — Dashboard/home page                                                    |
| `workspace.tsx` | `/workspaces/:wid` — Workspace detail                                        |
| `settings.tsx`  | `/workspaces/:wid/settings` — Workspace settings                             |
| `sessions.tsx`  | `/workspaces/:wid/sessions` — Session list                                   |
| `session.tsx`   | `/workspaces/:wid/sessions/:sid` — Session chat view (main interaction page) |
| `boards.tsx`    | `/workspaces/:wid/boards` — Board list                                       |
| `board.tsx`     | `/workspaces/:wid/boards/:bid` — Board detail (wrapper)                      |
| `codebases.tsx` | `/workspaces/:wid/codebases` — Codebase list                                 |
| `codebase.tsx`  | `/workspaces/:wid/codebases/:cid` — Codebase detail                          |
| `not-found.tsx` | 404 catch-all                                                                |

## board/ subdirectory (Kanban Board Components)

| File                    | Contains                                                 |
| ----------------------- | -------------------------------------------------------- |
| `board-container.tsx`   | Main kanban board layout with @dnd-kit DragOverlay       |
| `board-column.tsx`      | Single column with droppable area + sortable task cards  |
| `kanban-card.tsx`       | Individual task card (draggable)                         |
| `task-detail-panel.tsx` | Slide-over panel showing task details, fields, artifacts |
| `task-status-chip.tsx`  | Status badge chip (todo/in_progress/review/done)         |
| `agent-pill.tsx`        | Specialist/agent indicator pill                          |
| `add-card-button.tsx`   | "Add card" trigger button in column footer               |
| `add-card-modal.tsx`    | Modal form for creating a new task                       |
| `add-column-button.tsx` | "Add column" trigger button                              |
| `add-column-modal.tsx`  | Modal form for creating a new column                     |

## session/ subdirectory

| File                  | Contains                                                     |
| --------------------- | ------------------------------------------------------------ |
| `journey-sidebar.tsx` | Collapsible sidebar showing decision timeline + file changes |

## Key Patterns

- All pages use TanStack Query hooks from `hooks/` for data fetching — no manual `fetch`
- Routing via `wouter` — `useParams()` for path params
- `@dnd-kit/core` + `@dnd-kit/sortable` for kanban drag-and-drop
- Tailwind CSS v4 for styling (`@import "tailwindcss"`)
- `api.ts` types imported from `lib/types.ts`

## Connections

- **Uses:** `hooks/` (data fetching), `lib/types.ts` (types), `lib/api.ts` (API client), `components/` (shared UI)
- **No backend dependencies** — pure React/TypeScript
