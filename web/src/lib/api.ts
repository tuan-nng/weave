import type {
  Board,
  BoardDetail,
  Codebase,
  CodebaseDetail,
  Column,
  CreateBoardRequest,
  CreateCardRequest,
  CreateCodebaseRequest,
  CreateColumnRequest,
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
  Task,
  TraceRow,
  UpdateBoardRequest,
  UpdateColumnRequest,
  UpdateTaskRequest,
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
    toolCalls: (sessionId: string) =>
      apiFetch<TraceRow[]>(`/api/sessions/${sessionId}/trace/tools`),
  },

  // Kanban (feat-026). Boards are workspace-scoped; the composite GET
  // returns BoardDetail `{board, columns[], tasks[]}`. Column and task
  // mutation paths drop the workspace id (the server resolves it from the
  // row); boards.get keeps it for the cross-workspace 404 guard.
  kanban: {
    boards: {
      list: (workspaceId: string) => apiFetch<Board[]>(`/api/workspaces/${workspaceId}/boards`),
      get: (workspaceId: string, boardId: string) =>
        apiFetch<BoardDetail>(`/api/workspaces/${workspaceId}/boards/${boardId}`),
      create: (workspaceId: string, data: CreateBoardRequest) =>
        apiFetch<Board>(`/api/workspaces/${workspaceId}/boards`, {
          method: "POST",
          body: JSON.stringify(data),
        }),
      update: (workspaceId: string, boardId: string, data: UpdateBoardRequest) =>
        apiFetch<Board>(`/api/workspaces/${workspaceId}/boards/${boardId}`, {
          method: "PATCH",
          body: JSON.stringify(data),
        }),
      delete: (workspaceId: string, boardId: string) =>
        apiFetch<null>(`/api/workspaces/${workspaceId}/boards/${boardId}`, {
          method: "DELETE",
        }),
    },
    columns: {
      create: (workspaceId: string, boardId: string, data: CreateColumnRequest) =>
        apiFetch<Column>(`/api/workspaces/${workspaceId}/boards/${boardId}/columns`, {
          method: "POST",
          body: JSON.stringify(data),
        }),
      update: (columnId: string, data: UpdateColumnRequest) =>
        apiFetch<Column>(`/api/columns/${columnId}`, {
          method: "PATCH",
          body: JSON.stringify(data),
        }),
    },
    cards: {
      create: (workspaceId: string, boardId: string, data: CreateCardRequest) =>
        apiFetch<Task>(`/api/workspaces/${workspaceId}/boards/${boardId}/cards`, {
          method: "POST",
          body: JSON.stringify(data),
        }),
    },
    tasks: {
      update: (taskId: string, data: UpdateTaskRequest) =>
        apiFetch<Task>(`/api/tasks/${taskId}`, {
          method: "PATCH",
          body: JSON.stringify(data),
        }),
      delete: (taskId: string) => apiFetch<null>(`/api/tasks/${taskId}`, { method: "DELETE" }),
    },
  },

  // Codebases (feat-032). List/create are workspace-scoped. The
  // composite GET and DELETE take the workspace id as a `?wid=`
  // query param so the server can run the cross-workspace 404 guard.
  codebases: {
    list: (workspaceId: string) => apiFetch<Codebase[]>(`/api/workspaces/${workspaceId}/codebases`),
    get: (workspaceId: string, codebaseId: string) =>
      apiFetch<CodebaseDetail>(
        `/api/codebases/${codebaseId}?wid=${encodeURIComponent(workspaceId)}`,
      ),
    create: (workspaceId: string, data: CreateCodebaseRequest) =>
      apiFetch<Codebase>(`/api/workspaces/${workspaceId}/codebases`, {
        method: "POST",
        body: JSON.stringify(data),
      }),
    delete: (workspaceId: string, codebaseId: string) =>
      apiFetch<null>(`/api/codebases/${codebaseId}?wid=${encodeURIComponent(workspaceId)}`, {
        method: "DELETE",
      }),
  },

  // Tasks (feat-053). The unbound endpoint serves the wizard's Step 4
  // task picker; we only expose the one query we need, with the
  // server-side `?unbound=true` filter baked in. Returns
  // `Task[]` (active + `session_id IS NULL` in the workspace).
  tasks: {
    unbound: (workspaceId: string) =>
      apiFetch<Task[]>(`/api/workspaces/${workspaceId}/tasks?unbound=true`),
  },
};
