import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { CreateProviderRequest } from "../lib/types";

export function useProviders() {
  return useQuery({
    queryKey: queryKeys.providers.list(),
    queryFn: () => api.providers.list(),
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
