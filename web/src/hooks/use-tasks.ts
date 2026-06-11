// use-tasks.ts — TanStack Query bindings for the tasks feature
// (feat-053). The wizard's Step 4 task picker is the only consumer of
// `useUnboundTasks`; the bound-task queries and per-task mutations
// continue to live alongside the board mutations in `use-board.ts` /
// `use-kanban.ts` (they're part of the kanban feature, not tasks).
//
// `staleTime` is 30s — the unbound list is a live operational surface
// (a session binding a task removes it from the response on the next
// poll). A longer staleTime would show a stale row to a user who just
// created a session that bound to it.

import { skipToken, useQuery } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { Task } from "../lib/types";

export function useUnboundTasks(workspaceId: string | null) {
  return useQuery({
    // `skipToken` is the v5-recommended way to opt out of a query
    // without materializing a cache key. Earlier drafts used a
    // `__dormant__` sentinel which wrote a real entry into the
    // cache and made the disabled-state visible in the devtools
    // cache tab as a non-zero row. With `skipToken` the hook is
    // truly inactive until `workspaceId` is a non-null string.
    queryKey: queryKeys.tasks.unbound(workspaceId ?? ""),
    queryFn: workspaceId ? () => api.tasks.unbound(workspaceId) : skipToken,
    staleTime: 30 * 1000, // 30s — see file header
  });
}

export type { Task };
