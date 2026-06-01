import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { CreateSessionRequest, CreateWorkspaceRequest } from "../lib/types";

export function useWorkspaces() {
  return useQuery({
    queryKey: queryKeys.workspaces.list(),
    queryFn: () => api.workspaces.list(),
  });
}

export function useCreateWorkspace() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (data: CreateWorkspaceRequest) => api.workspaces.create(data),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.workspaces.all() });
    },
  });
}

export function useRenameWorkspace() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) => api.workspaces.update(id, { name }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.workspaces.all() });
    },
  });
}

export function useDeleteWorkspace() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api.workspaces.delete(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.workspaces.all() });
    },
  });
}

export function useWorkspaceSessions(workspaceId: string) {
  return useQuery({
    queryKey: queryKeys.sessions.list(workspaceId),
    queryFn: () => api.sessions.list(workspaceId),
  });
}

export function useCreateSession(workspaceId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (data: CreateSessionRequest) => api.sessions.create(workspaceId, data),
    onSuccess: () => {
      qc.invalidateQueries({
        queryKey: queryKeys.sessions.list(workspaceId),
      });
    },
  });
}
