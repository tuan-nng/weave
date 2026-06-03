import { createBrowserRouter } from "react-router";
import AppLayout from "./layout";
import HomePage from "./pages/home";
import WorkspacePage from "./pages/workspace";
import SessionsPage from "./pages/sessions";
import SessionPage from "./pages/session";
import SettingsPage from "./pages/settings";
import BoardsPage from "./pages/boards";
import BoardPage from "./pages/board";
import CodebasesPage from "./pages/codebases";
import CodebasePage from "./pages/codebase";
import NotFoundPage from "./pages/not-found";

export const router = createBrowserRouter([
  {
    element: <AppLayout />,
    children: [
      { index: true, element: <HomePage /> },
      { path: "workspaces/:id", element: <WorkspacePage /> },
      { path: "sessions", element: <SessionsPage /> },
      { path: "sessions/:id", element: <SessionPage /> },
      // Kanban (feat-026): top-level list + workspace-scoped detail.
      { path: "boards", element: <BoardsPage /> },
      { path: "workspaces/:wid/boards/:bid", element: <BoardPage /> },
      // Codebases (feat-032): same shape as kanban.
      { path: "codebases", element: <CodebasesPage /> },
      { path: "workspaces/:wid/codebases/:cid", element: <CodebasePage /> },
      { path: "settings", element: <SettingsPage /> },
      { path: "*", element: <NotFoundPage /> },
    ],
  },
]);
