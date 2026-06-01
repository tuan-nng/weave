import type { PaginationParams } from "./types";

export const queryKeys = {
  all: ["api"] as const,

  health: () => [...queryKeys.all, "health"] as const,

  workspaces: {
    all: () => [...queryKeys.all, "workspaces"] as const,
    list: (params?: PaginationParams) => [...queryKeys.workspaces.all(), "list", params] as const,
    detail: (id: string) => [...queryKeys.workspaces.all(), "detail", id] as const,
  },

  providers: {
    all: () => [...queryKeys.all, "providers"] as const,
    list: () => [...queryKeys.providers.all(), "list"] as const,
    models: (id: string) => [...queryKeys.providers.all(), "models", id] as const,
  },

  sessions: {
    all: () => [...queryKeys.all, "sessions"] as const,
    list: (workspaceId: string, params?: PaginationParams) =>
      [...queryKeys.sessions.all(), "list", workspaceId, params] as const,
    detail: (id: string) => [...queryKeys.sessions.all(), "detail", id] as const,
    history: (sessionId: string, params?: PaginationParams) =>
      [...queryKeys.sessions.all(), "history", sessionId, params] as const,
  },

  traces: {
    all: () => [...queryKeys.all, "traces"] as const,
    list: (sessionId: string) => [...queryKeys.traces.all(), "list", sessionId] as const,
    journey: (sessionId: string) => [...queryKeys.traces.all(), "journey", sessionId] as const,
    fileChanges: (sessionId: string) =>
      [...queryKeys.traces.all(), "fileChanges", sessionId] as const,
  },
} as const;
