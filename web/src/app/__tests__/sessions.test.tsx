// Page-level tests for the Sessions list page (feat-061).
//
// Mirrors `codebases.test.tsx`: mock the `api` surface, mount the page
// with a real QueryClient + MemoryRouter, assert rendered content and
// the new `+ New Session` per-workspace flow.
//
// Coverage:
//   1. "No workspaces" empty state (regression guard).
//   2. Per-workspace `+ New Session` button visible even with zero sessions.
//   3. Existing session rows still render alongside the per-workspace button.
//   4. Clicking the button opens the NewSessionModal pre-bound to that workspace.
//   5. Submitting the form calls `api.sessions.create` with the right
//      payload (provider_id + specialist_id) and navigates to `/sessions/:id`.
//
// No SSE is involved on this page, so no `EventSource` stub is needed.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryRouter, RouterProvider } from "react-router";
import SessionsPage from "../pages/sessions";

vi.mock("../../lib/api", () => ({
  api: {
    workspaces: {
      list: vi.fn(),
    },
    sessions: {
      list: vi.fn(),
      create: vi.fn(),
    },
    providers: {
      list: vi.fn(),
    },
    specialists: {
      list: vi.fn(),
    },
    codebases: {
      list: vi.fn(),
    },
  },
}));

import { api } from "../../lib/api";
const mockApi = vi.mocked(api);

const mockWorkspaces = [
  {
    id: "w1",
    name: "Default",
    status: "active",
    is_default: true,
    created_at: "2026-06-01T00:00:00Z",
    updated_at: "2026-06-01T00:00:00Z",
  },
  {
    id: "w2",
    name: "Backend",
    status: "active",
    is_default: false,
    created_at: "2026-06-01T00:00:00Z",
    updated_at: "2026-06-01T00:00:00Z",
  },
];

const mockSessions = [
  {
    id: "s1",
    workspace_id: "w1",
    provider_id: "p1",
    specialist_id: "dev-crafter",
    parent_session_id: null,
    status: "completed",
    model: "claude-sonnet-4-5",
    cwd: null,
    codebase_id: null,
    created_at: "2026-06-01T00:00:00Z",
    updated_at: "2026-06-01T00:00:00Z",
  },
];

const mockProviders = [
  { id: "p1", type: "anthropic", name: "Anthropic", created_at: "2026-06-01T00:00:00Z" },
  { id: "p2", type: "openai", name: "OpenAI", created_at: "2026-06-01T00:00:00Z" },
];

const mockSpecialists = [
  {
    name: "dev-crafter",
    description: "Writes code",
    model: null,
    tool_profile: "implementation",
    tags: [],
  },
  {
    name: "review-guard",
    description: "Reviews code",
    model: null,
    tool_profile: "review",
    tags: [],
  },
];

function renderList() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  // The router intentionally has no `/sessions/:id` route — the
  // post-create navigate is asserted via `router.state.location.pathname`
  // only. The 404 the unknown route logs is expected and does not
  // affect the test outcome. (We can't add a catch-all here without
  // breaking the navigate call from the post-create onSuccess.)
  const router = createMemoryRouter([{ path: "/sessions", element: <SessionsPage /> }], {
    initialEntries: ["/sessions"],
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router };
}

beforeEach(() => {
  vi.clearAllMocks();
  // Silence the React Router 404 that the unknown `/sessions/:id`
  // route logs during the create-and-navigate flow. The 404 is
  // expected (we deliberately do not mount the detail page) and
  // does not affect the test outcome.
  vi.spyOn(console, "error").mockImplementation(() => {});
  mockApi.workspaces.list.mockResolvedValue({ data: mockWorkspaces });
  // Default: w1 has a session, w2 has none. Each test can override.
  mockApi.sessions.list.mockImplementation(async (wsid: string) =>
    wsid === "w1" ? { data: mockSessions } : { data: [] },
  );
  mockApi.providers.list.mockResolvedValue(mockProviders);
  mockApi.specialists.list.mockResolvedValue(mockSpecialists);
  // Default: no codebases registered. Each test that exercises the
  // codebase picker can override with `mockApi.codebases.list.mockResolvedValueOnce(...)`.
  mockApi.codebases.list.mockResolvedValue({ data: [] });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("sessions list", () => {
  it("renders the no-workspaces empty state when there are none", async () => {
    mockApi.workspaces.list.mockResolvedValueOnce({ data: [] });
    renderList();
    expect(await screen.findByText("No workspaces")).toBeInTheDocument();
    expect(screen.getByText(/create a workspace first to start sessions/i)).toBeInTheDocument();
    // The per-workspace button is not rendered when there are no workspaces.
    expect(screen.queryByRole("button", { name: /\+ New Session in/i })).not.toBeInTheDocument();
  });

  it("renders a + New Session button per workspace, even when a workspace has zero sessions", async () => {
    renderList();
    // Both workspaces get the button — w1 has a session, w2 has none.
    // The empty-workspace case is the feat-061 acceptance requirement.
    expect(
      await screen.findByRole("button", { name: /\+ New Session in Default/i }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /\+ New Session in Backend/i })).toBeInTheDocument();
  });

  it("renders existing session rows alongside the per-workspace button", async () => {
    renderList();
    // The session for w1 is rendered.
    expect(await screen.findByText("dev-crafter")).toBeInTheDocument();
    // The w1 button is still there.
    expect(screen.getByRole("button", { name: /\+ New Session in Default/i })).toBeInTheDocument();
    // The w2 button is there too (no sessions for w2).
    expect(screen.getByRole("button", { name: /\+ New Session in Backend/i })).toBeInTheDocument();
  });

  it("clicking the per-workspace button opens the NewSessionModal", async () => {
    renderList();
    const button = await screen.findByRole("button", {
      name: /\+ New Session in Backend/i,
    });
    fireEvent.click(button);
    // The modal header appears.
    expect(await screen.findByRole("heading", { name: "New Session" })).toBeInTheDocument();
    // The provider select is present and has the providers as options.
    const providerSelect = screen.getByDisplayValue("Select provider…") as HTMLSelectElement;
    expect(providerSelect).toBeInTheDocument();
    expect(providerSelect.querySelectorAll("option")).toHaveLength(1 + mockProviders.length);
    // The specialist select is present with "No specialist" + specialists.
    const specialistSelect = screen.getByDisplayValue("No specialist") as HTMLSelectElement;
    expect(specialistSelect).toBeInTheDocument();
    expect(specialistSelect.querySelectorAll("option")).toHaveLength(1 + mockSpecialists.length);
  });

  it("submitting the form creates a session and navigates to /sessions/:id", async () => {
    const newSession = {
      id: "new-session-id",
      workspace_id: "w2",
      provider_id: "p1",
      specialist_id: "dev-crafter",
      parent_session_id: null,
      status: "connecting",
      model: null,
      cwd: null,
      codebase_id: null,
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-01T00:00:00Z",
    };
    mockApi.sessions.create.mockResolvedValueOnce(newSession);

    const { router } = renderList();

    // Open the modal for w2.
    const button = await screen.findByRole("button", {
      name: /\+ New Session in Backend/i,
    });
    fireEvent.click(button);

    // Pick the provider (Backend has no sessions, so the form mounts with
    // empty defaults — no async query race).
    const providerSelect = (await screen.findByDisplayValue(
      "Select provider…",
    )) as HTMLSelectElement;
    fireEvent.change(providerSelect, { target: { value: "p1" } });

    // Pick the specialist.
    const specialistSelect = screen.getByDisplayValue("No specialist") as HTMLSelectElement;
    fireEvent.change(specialistSelect, { target: { value: "dev-crafter" } });

    // Submit.
    const submit = screen.getByRole("button", { name: /create session/i });
    fireEvent.click(submit);

    // The mutation runs and onSuccess fires; the modal closes and
    // the page navigates.
    await waitFor(() => {
      expect(mockApi.sessions.create).toHaveBeenCalledWith("w2", {
        provider_id: "p1",
        specialist_id: "dev-crafter",
      });
    });
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/sessions/new-session-id");
    });
  });

  it("the codebase picker shows a disabled empty state with a /codebases link when no codebases are registered", async () => {
    // Default beforeEach: mockApi.codebases.list returns { data: [] }
    renderList();
    const button = await screen.findByRole("button", {
      name: /\+ New Session in Backend/i,
    });
    fireEvent.click(button);

    // The dropdown is disabled and reads "No codebases registered".
    const disabledSelect = (await screen.findByDisplayValue(
      "No codebases registered",
    )) as HTMLSelectElement;
    expect(disabledSelect).toBeInTheDocument();
    expect(disabledSelect).toBeDisabled();
    // The hint copy points at /codebases.
    expect(screen.getByText(/register a codebase/i)).toBeInTheDocument();
  });

  it("submitting with a selected codebase sends codebase_id in the create payload", async () => {
    const mockCodebases = [
      {
        id: "cb1",
        workspace_id: "w2",
        path: "/home/u/repo-a",
        branch: null,
        label: "Repo A",
        created_at: "2026-06-01T00:00:00Z",
      },
      {
        id: "cb2",
        workspace_id: "w2",
        path: "/home/u/repo-b",
        branch: null,
        label: "Repo B",
        created_at: "2026-06-01T00:00:00Z",
      },
    ];
    mockApi.codebases.list.mockResolvedValueOnce({ data: mockCodebases });

    const newSession = {
      id: "new-session-id-2",
      workspace_id: "w2",
      provider_id: "p1",
      specialist_id: null,
      parent_session_id: null,
      status: "connecting",
      model: null,
      cwd: "/home/u/repo-b",
      codebase_id: "cb2",
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-01T00:00:00Z",
    };
    mockApi.sessions.create.mockResolvedValueOnce(newSession);

    const { router } = renderList();
    const button = await screen.findByRole("button", {
      name: /\+ New Session in Backend/i,
    });
    fireEvent.click(button);

    // Wait for the codebase list query to populate the dropdown.
    const codebaseSelect = (await screen.findByDisplayValue(
      "No codebase (operate in workspace root)",
    )) as HTMLSelectElement;
    expect(codebaseSelect.querySelectorAll("option")).toHaveLength(1 + mockCodebases.length);

    // Pick the provider, the codebase, submit.
    const providerSelect = screen.getByDisplayValue("Select provider…") as HTMLSelectElement;
    fireEvent.change(providerSelect, { target: { value: "p1" } });
    fireEvent.change(codebaseSelect, { target: { value: "cb2" } });

    const submit = screen.getByRole("button", { name: /create session/i });
    fireEvent.click(submit);

    await waitFor(() => {
      expect(mockApi.sessions.create).toHaveBeenCalledWith("w2", {
        provider_id: "p1",
        codebase_id: "cb2",
      });
    });
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/sessions/new-session-id-2");
    });
  });
});
