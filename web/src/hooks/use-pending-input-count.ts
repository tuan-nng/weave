// usePendingInputCount — sum of awaiting-input sessions across all
// workspaces the user can see. Drives the Sessions nav badge
// (F-16): when count > 0, render a small rose pill next to the
// "Sessions" label so the operator can spot a paused agent without
// clicking into the page.
//
// Strategy: one query per workspace (parallel via TanStack Query's
// normal cache), then sum the lengths in a `useMemo`. Cheap — the
// backend returns just the rows with `awaiting_user_input=true` for
// the given workspace. Re-fetches every 10 seconds so a session
// that finishes while the nav is open eventually drops off the
// badge without a page reload.

import { useQueries } from "@tanstack/react-query";
import { useMemo } from "react";
import { useWorkspaces } from "./use-workspaces";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";

export function usePendingInputCount(): { count: number; isLoading: boolean } {
  const { data: workspacesResp } = useWorkspaces();
  const workspaces = workspacesResp?.data ?? [];

  const queries = useQueries({
    queries: workspaces.map((ws) => ({
      queryKey: queryKeys.sessions.awaitingInput(ws.id),
      queryFn: () => api.sessions.awaitingInput(ws.id),
      // The session SSE stream is the authoritative source; this is
      // just a snapshot. 10s is the cheapest cadence that still feels
      // responsive when a session finishes a turn.
      staleTime: 10_000,
      refetchInterval: 10_000,
      refetchOnWindowFocus: false,
    })),
  });

  const isLoading = queries.some((q) => q.isLoading);
  const count = useMemo(
    () => queries.reduce((acc, q) => acc + (q.data?.length ?? 0), 0),
    [queries],
  );

  return { count, isLoading };
}
