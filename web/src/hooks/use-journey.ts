import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { TraceRow } from "../lib/types";

/// TanStack Query wrapper for `/api/sessions/:sid/trace/journey`.
///
/// The journey endpoint returns `decision` + `error` events (see
/// `TraceStore::list_journey` on the backend). `useSession` invalidates
/// this query on every `message_persisted` SSE event so the sidebar
/// stays in sync with the chat without a separate live channel.
export function useJourney(sessionId: string): UseQueryResult<TraceRow[]> {
  return useQuery({
    queryKey: queryKeys.traces.journey(sessionId),
    queryFn: () => api.traces.journey(sessionId),
    enabled: Boolean(sessionId),
  });
}
