import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import HomePage from "../pages/home";
import AppLayout from "../layout";

// Mock the API module
vi.mock("../../lib/api", () => ({
  api: {
    workspaces: {
      list: vi.fn(),
      create: vi.fn(),
      update: vi.fn(),
      delete: vi.fn(),
    },
  },
}));

import { api } from "../../lib/api";
const mockApi = vi.mocked(api);

function renderHome() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        element: <AppLayout />,
        children: [{ index: true, element: <HomePage /> }],
      },
    ],
    { initialEntries: ["/"] },
  );

  return {
    ...render(
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>,
    ),
    queryClient,
  };
}

describe("HomePage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders the workspaces heading", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: [] });
    renderHome();
    expect(await screen.findByRole("heading", { name: "Workspaces" })).toBeInTheDocument();
  });

  it("shows empty state when no workspaces", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: [] });
    renderHome();
    expect(await screen.findByText("No workspaces")).toBeInTheDocument();
    expect(screen.getByText("Create your first workspace to get started")).toBeInTheDocument();
  });

  it("renders workspace list", async () => {
    mockApi.workspaces.list.mockResolvedValue({
      data: [
        {
          id: "ws-1",
          name: "My Workspace",
          status: "active",
          is_default: false,
          created_at: "2025-01-15T00:00:00Z",
          updated_at: "2025-01-15T00:00:00Z",
        },
      ],
    });
    renderHome();
    expect(await screen.findByText("My Workspace")).toBeInTheDocument();
  });

  it("shows default badge for default workspace", async () => {
    mockApi.workspaces.list.mockResolvedValue({
      data: [
        {
          id: "default",
          name: "default",
          status: "active",
          is_default: true,
          created_at: "2025-01-15T00:00:00Z",
          updated_at: "2025-01-15T00:00:00Z",
        },
      ],
    });
    renderHome();
    expect(await screen.findByText("★ Default")).toBeInTheDocument();
  });

  it("shows create form", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: [] });
    renderHome();
    expect(await screen.findByPlaceholderText("Enter workspace name...")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create" })).toBeInTheDocument();
  });
});
