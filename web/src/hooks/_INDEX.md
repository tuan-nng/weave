# web/src/hooks/ — TanStack Query Data Hooks

React hooks wrapping TanStack Query for all server-state fetching and mutations. No manual `fetch` in components — everything goes through these hooks.

## Files

| File                  | Size | Contains                                                                                                                                                                                                                                              |
| --------------------- | ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `use-session.ts`      | 19KB | `useSession(id)`, `useSessionHistory(id)`, `useCreateSession()`, `useSendPrompt()`, `useCancelSession()`, `useSessionStream(id)` (SSE via EventSource), `useSessionJourney(id)`, `useSessionTrace(id)`, `useSessionFileChanges(id)`                   |
| `use-board.ts`        | 14KB | `useBoard(id)`, `useListBoards(wsId)`, `useCreateBoard()`, `useUpdateBoard()`, `useDeleteBoard()`, `useCreateColumn()`, `useUpdateColumn()`, `useDeleteColumn()`, `useCreateCard()`, `useUpdateTask()`, `useDeleteTask()`, `useBoardStream(id)` (SSE) |
| `use-workspaces.ts`   | 2KB  | `useWorkspaces(params)`, `useWorkspace(id)`, `useCreateWorkspace()`, `useUpdateWorkspace()`, `useDeleteWorkspace()`                                                                                                                                   |
| `use-codebase.ts`     | 2KB  | `useCodebases(wsId)`, `useCodebase(id)`, `useCreateCodebase()`, `useDeleteCodebase()`                                                                                                                                                                 |
| `use-providers.ts`    | 1KB  | `useProviders(wsId)`, `useCreateProvider()`, `useDeleteProvider()`, `useProviderModels(id)`                                                                                                                                                           |
| `use-file-changes.ts` | 1KB  | `useFileChanges(sessionId)`                                                                                                                                                                                                                           |
| `use-journey.ts`      | 1KB  | `useJourney(sessionId)`                                                                                                                                                                                                                               |
| `use-specialists.ts`  | 1KB  | `useSpecialists()`                                                                                                                                                                                                                                    |

## Key Patterns

- `useQuery` for reads, `useMutation` with `queryClient.invalidateQueries` for writes
- Query key factory in `lib/query-keys.ts` — structured hierarchical keys
- SSE hooks (`useSessionStream`, `useBoardStream`) use `EventSource` with `Last-Event-ID` for reconnect
- Optimistic updates: mutations immediately update cache, rollback on error
- All hooks accept `workspace_id` where required (matches backend scoping)

## Connections

- **Uses:** `lib/api.ts` (HTTP calls), `lib/query-keys.ts` (cache keys), `lib/types.ts` (types)
- **Used by:** `app/pages/*` components
