export const ROUTES = {
  home: "/",
  workspace: (id: string) => `/workspaces/${id}`,
  session: (id: string) => `/sessions/${id}`,
  settings: "/settings",
} as const;
