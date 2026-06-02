import { useQuery } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";

/// List specialists loaded from `resources/specialists/*.md` on the
/// backend. Used by the AddColumnModal to pick a specialist when
/// `auto_trigger=true`.
export function useSpecialists() {
  return useQuery({
    queryKey: queryKeys.specialists.list(),
    queryFn: () => api.specialists.list(),
    staleTime: 5 * 60 * 1000, // 5 min — specialists rarely change
  });
}
