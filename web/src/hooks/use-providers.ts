import { skipToken, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { CreateProviderRequest, ModelInfo } from "../lib/types";

export function useProviders() {
  return useQuery({
    queryKey: queryKeys.providers.list(),
    queryFn: () => api.providers.list(),
  });
}

/// feat-053: per-provider model list. The wizard's Step 3 pre-selects
/// the first entry on provider change. `staleTime` mirrors the CLI
/// `ModelCache` (5 min) on the backend — see feat-042.
///
/// `skipToken` keeps the cache quiet when the hook is dormant
/// (no provider picked yet). Without it, the `__dormant__` sentinel
/// would write a real entry into the cache.
export function useProviderModels(providerId: string | null) {
  return useQuery({
    queryKey: queryKeys.providers.models(providerId ?? ""),
    queryFn: providerId ? () => api.providers.models(providerId) : skipToken,
    staleTime: 5 * 60 * 1000,
  });
}

export function useCreateProvider() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (data: CreateProviderRequest) => api.providers.create(data),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.providers.all() });
    },
  });
}

export function useDeleteProvider() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.providers.delete(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.providers.all() });
    },
  });
}

export type { ModelInfo };
