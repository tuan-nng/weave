import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { TraceRow } from "../lib/types";

/// TanStack Query wrapper for `/api/sessions/:sid/trace/tools`.
///
/// The tools endpoint returns `tool_call` events (see
/// `TraceStore::list_tool_calls` on the backend). `useSession`
/// invalidates this query on every `message_persisted` SSE event so
/// the sidebar stays in sync with the chat without a separate live
/// channel. Companion to `useJourney` / `useFileChanges`.
export function useToolCalls(sessionId: string): UseQueryResult<TraceRow[]> {
  return useQuery({
    queryKey: queryKeys.traces.toolCalls(sessionId),
    queryFn: () => api.traces.toolCalls(sessionId),
    enabled: Boolean(sessionId),
  });
}
