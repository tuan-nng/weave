// Hooks for the codebases feature (feat-032). Mirrors the shape of
// `use-board.ts` / `use-workspaces.ts` but keeps the codebases surface
// intentionally thin — no SSE, no reducer. The composite GET is a
// one-shot query; create/delete invalidate the workspace's list.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { Codebase, CodebaseDetail, CreateCodebaseRequest } from "../lib/types";

export function useCodebases(workspaceId: string) {
  return useQuery({
    queryKey: queryKeys.codebases.list(workspaceId),
    queryFn: () => api.codebases.list(workspaceId),
    enabled: Boolean(workspaceId),
  });
}

export function useCodebase(workspaceId: string, codebaseId: string) {
  return useQuery({
    queryKey: queryKeys.codebases.detail(workspaceId, codebaseId),
    queryFn: () => api.codebases.get(workspaceId, codebaseId),
    enabled: Boolean(workspaceId) && Boolean(codebaseId),
  });
}

export function useCreateCodebase(workspaceId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (data: CreateCodebaseRequest) => api.codebases.create(workspaceId, data),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.codebases.list(workspaceId) });
    },
  });
}

export function useDeleteCodebase(workspaceId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (codebaseId: string) => api.codebases.delete(workspaceId, codebaseId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.codebases.list(workspaceId) });
    },
  });
}

export type { Codebase, CodebaseDetail, CreateCodebaseRequest };
