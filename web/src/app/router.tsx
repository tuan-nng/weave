import { createBrowserRouter } from "react-router";
import AppLayout from "./layout";
import HomePage from "./pages/home";
import WorkspacePage from "./pages/workspace";
import SessionsPage from "./pages/sessions";
import SessionPage from "./pages/session";
import SettingsPage from "./pages/settings";
import NotFoundPage from "./pages/not-found";

export const router = createBrowserRouter([
  {
    element: <AppLayout />,
    children: [
      { index: true, element: <HomePage /> },
      { path: "workspaces/:id", element: <WorkspacePage /> },
      { path: "sessions", element: <SessionsPage /> },
      { path: "sessions/:id", element: <SessionPage /> },
      { path: "settings", element: <SettingsPage /> },
      { path: "*", element: <NotFoundPage /> },
    ],
  },
]);
