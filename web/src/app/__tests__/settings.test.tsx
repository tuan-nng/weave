import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import SettingsPage from "../pages/settings";
import AppLayout from "../layout";

// Mock the API module
vi.mock("../../lib/api", () => ({
  api: {
    providers: {
      list: vi.fn(),
      create: vi.fn(),
      delete: vi.fn(),
    },
  },
}));

import { api } from "../../lib/api";
const mockApi = vi.mocked(api);

function renderSettings() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        element: <AppLayout />,
        children: [{ path: "settings", element: <SettingsPage /> }],
      },
    ],
    { initialEntries: ["/settings"] },
  );

  return render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

describe("SettingsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders the settings heading", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    expect(await screen.findByRole("heading", { name: "Settings" })).toBeInTheDocument();
  });

  it("shows empty state when no providers", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    expect(await screen.findByText("No providers")).toBeInTheDocument();
    expect(screen.getByText("Add an Anthropic provider to get started")).toBeInTheDocument();
  });

  it("renders provider list", async () => {
    mockApi.providers.list.mockResolvedValue([
      {
        id: "p-1",
        name: "Production",
        type: "anthropic",
        created_at: "2025-01-15T00:00:00Z",
      },
    ]);
    renderSettings();
    expect(await screen.findByText("Production")).toBeInTheDocument();
  });

  it("shows add provider form", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    expect(await screen.findByRole("heading", { name: "Add Provider" })).toBeInTheDocument();
    expect(screen.getByPlaceholderText("e.g. Production")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("sk-ant-...")).toBeInTheDocument();
  });
});
