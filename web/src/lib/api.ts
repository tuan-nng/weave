import type {
  CreateProviderRequest,
  CreateSessionRequest,
  CreateWorkspaceRequest,
  FileChangeSummary,
  HealthResponse,
  ModelInfo,
  PaginatedResponse,
  PaginationParams,
  Provider,
  Session,
  Message,
  SpecialistInfo,
  TraceRow,
  UpdateWorkspaceRequest,
  Workspace,
} from "./types";

// ---------------------------------------------------------------------------
// Error class
// ---------------------------------------------------------------------------

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

// ---------------------------------------------------------------------------
// Base fetch helper — unwraps {data: T} / {error: {code, message}} envelopes
// ---------------------------------------------------------------------------

async function apiFetch<T>(path: string, options?: RequestInit): Promise<T> {
  const method = options?.method?.toUpperCase();
  const isBodyRequest = method === "POST" || method === "PATCH" || method === "PUT";

  const res = await fetch(path, {
    ...options,
    headers: {
      ...(isBodyRequest ? { "Content-Type": "application/json" } : {}),
      ...options?.headers,
    },
  });

  let json: unknown;
  try {
    json = await res.json();
  } catch {
    throw new ApiError(
      res.status,
      "invalid_response",
      `Server returned non-JSON response (status ${res.status})`,
    );
  }

  if (!res.ok) {
    const err = json as { error?: { code: string; message: string } };
    throw new ApiError(
      res.status,
      err.error?.code ?? "unknown",
      err.error?.message ?? `Request failed with status ${res.status}`,
    );
  }

  // Health endpoint returns flat object (no data envelope)
  if (path.startsWith("/api/health")) {
    return json as T;
  }

  return (json as { data: T }).data;
}

// ---------------------------------------------------------------------------
// Query-string helper for pagination params
// ---------------------------------------------------------------------------

function qs(params?: PaginationParams): string {
  if (!params) return "";
  const parts: string[] = [];
  if (params.cursor) parts.push(`cursor=${encodeURIComponent(params.cursor)}`);
  if (params.limit) parts.push(`limit=${params.limit}`);
  return parts.length ? `?${parts.join("&")}` : "";
}

// ---------------------------------------------------------------------------
// Typed API client
// ---------------------------------------------------------------------------

export const api = {
  // Health
  health: () => apiFetch<HealthResponse>("/api/health"),

  // Workspaces
  workspaces: {
    list: (params?: PaginationParams) =>
      apiFetch<PaginatedResponse<Workspace>>(`/api/workspaces${qs(params)}`),
    get: (id: string) => apiFetch<Workspace>(`/api/workspaces/${id}`),
    create: (data: CreateWorkspaceRequest) =>
      apiFetch<Workspace>("/api/workspaces", {
        method: "POST",
        body: JSON.stringify(data),
      }),
    update: (id: string, data: UpdateWorkspaceRequest) =>
      apiFetch<Workspace>(`/api/workspaces/${id}`, {
        method: "PATCH",
        body: JSON.stringify(data),
      }),
    delete: (id: string) => apiFetch<null>(`/api/workspaces/${id}`, { method: "DELETE" }),
  },

  // Providers
  providers: {
    list: () => apiFetch<Provider[]>("/api/providers"),
    create: (data: CreateProviderRequest) =>
      apiFetch<Provider>("/api/providers", {
        method: "POST",
        body: JSON.stringify(data),
      }),
    delete: (id: string) => apiFetch<null>(`/api/providers/${id}`, { method: "DELETE" }),
    models: (id: string) => apiFetch<ModelInfo[]>(`/api/providers/${id}/models`),
  },

  // Specialists
  specialists: {
    list: () => apiFetch<SpecialistInfo[]>("/api/specialists"),
  },

  // Sessions
  sessions: {
    list: (workspaceId: string, params?: PaginationParams) =>
      apiFetch<PaginatedResponse<Session>>(`/api/workspaces/${workspaceId}/sessions${qs(params)}`),
    get: (id: string) => apiFetch<Session>(`/api/sessions/${id}`),
    create: (workspaceId: string, data: CreateSessionRequest) =>
      apiFetch<Session>(`/api/workspaces/${workspaceId}/sessions`, {
        method: "POST",
        body: JSON.stringify(data),
      }),
    updateStatus: (id: string, status: string) =>
      apiFetch<Session>(`/api/sessions/${id}`, {
        method: "PATCH",
        body: JSON.stringify({ status }),
      }),
    delete: (id: string) => apiFetch<null>(`/api/sessions/${id}`, { method: "DELETE" }),
    history: (sessionId: string, params?: PaginationParams) =>
      apiFetch<PaginatedResponse<Message>>(`/api/sessions/${sessionId}/history${qs(params)}`),
    sendPrompt: (sessionId: string, prompt: string) =>
      apiFetch<{ message_id: string }>(`/api/sessions/${sessionId}/prompt`, {
        method: "POST",
        body: JSON.stringify({ prompt }),
      }),
    cancel: (sessionId: string) =>
      apiFetch<{ status: string }>(`/api/sessions/${sessionId}/cancel`, {
        method: "POST",
      }),
  },

  // Traces
  traces: {
    list: (sessionId: string) => apiFetch<TraceRow[]>(`/api/sessions/${sessionId}/trace`),
    journey: (sessionId: string) =>
      apiFetch<TraceRow[]>(`/api/sessions/${sessionId}/trace/journey`),
    fileChanges: (sessionId: string) =>
      apiFetch<FileChangeSummary[]>(`/api/sessions/${sessionId}/trace/files`),
  },
};
