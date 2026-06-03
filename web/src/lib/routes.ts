export const ROUTES = {
  home: "/",
  workspace: (id: string) => `/workspaces/${id}`,
  sessions: "/sessions",
  session: (id: string) => `/sessions/${id}`,
  // Kanban (feat-026): list page is a top-level route (mirrors `/sessions`),
  // detail page is workspace-scoped because the backend's composite endpoint
  // and lane automation both take the workspace id in the URL.
  boards: "/boards",
  board: (workspaceId: string, boardId: string) => `/workspaces/${workspaceId}/boards/${boardId}`,
  // Codebases (feat-032): same shape as boards — top-level list, workspace-scoped
  // detail at `/workspaces/:wid/codebases/:cid`.
  codebases: "/codebases",
  codebase: (workspaceId: string, codebaseId: string) =>
    `/workspaces/${workspaceId}/codebases/${codebaseId}`,
  settings: "/settings",
} as const;
