export const ROUTES = {
  home: "/",
  workspace: (id: string) => `/workspaces/${id}`,
  sessions: "/sessions",
  session: (id: string) => `/sessions/${id}`,
  settings: "/settings",
} as const;
