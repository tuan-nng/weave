import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { FileChangeSummary } from "../lib/types";

/// TanStack Query wrapper for `/api/sessions/:sid/trace/files`.
///
/// Returns deduplicated file changes grouped by path, with a
/// distinct-list of `actions` and a total `count` per path. Like
/// `useJourney`, this is invalidated on every `message_persisted`
/// SSE event from `useSession`.
export function useFileChanges(sessionId: string): UseQueryResult<FileChangeSummary[]> {
  return useQuery({
    queryKey: queryKeys.traces.fileChanges(sessionId),
    queryFn: () => api.traces.fileChanges(sessionId),
    enabled: Boolean(sessionId),
  });
}
