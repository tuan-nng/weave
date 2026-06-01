import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import AppLayout from "../layout";
import HomePage from "../pages/home";
import SettingsPage from "../pages/settings";
import NotFoundPage from "../pages/not-found";

// Mock the API module
vi.mock("../../lib/api", () => ({
  api: {
    workspaces: { list: vi.fn().mockResolvedValue({ data: [] }) },
    providers: { list: vi.fn().mockResolvedValue([]) },
  },
}));

function renderWithRouter(path: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        element: <AppLayout />,
        children: [
          { index: true, element: <HomePage /> },
          { path: "settings", element: <SettingsPage /> },
          { path: "*", element: <NotFoundPage /> },
        ],
      },
    ],
    { initialEntries: [path] },
  );

  return render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("App shell", () => {
  it("renders home page at /", async () => {
    renderWithRouter("/");
    expect(await screen.findByRole("heading", { name: "Workspaces" })).toBeInTheDocument();
    expect(screen.getByText("Weave")).toBeInTheDocument();
  });

  it("renders settings page at /settings", async () => {
    renderWithRouter("/settings");
    expect(await screen.findByRole("heading", { name: "Settings" })).toBeInTheDocument();
  });

  it("renders 404 for unknown routes", () => {
    renderWithRouter("/nonexistent");
    expect(screen.getByText("404")).toBeInTheDocument();
    expect(screen.getByText("Go home")).toBeInTheDocument();
  });

  it("has navigation links in sidebar", async () => {
    renderWithRouter("/");
    expect(screen.getByRole("link", { name: "Home" })).toHaveAttribute("href", "/");
    expect(screen.getByRole("link", { name: "Settings" })).toHaveAttribute("href", "/settings");
  });
});
