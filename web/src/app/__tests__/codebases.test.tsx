// Page-level tests for the Codebases list + detail pages (feat-032).
// Mirrors the structure of `kanban-board.test.tsx`: mock the `api`
// surface, mount the pages with a real QueryClient + MemoryRouter,
// assert rendered content. The test suite covers three flows:
//
//   1. List page renders codebases grouped by workspace and surfaces
//      a friendly empty state when a workspace has no codebases.
//   2. Detail page renders the composite response (row + git status).
//   3. Detail page surfaces `git_error` as a banner when the path is
//      not a git repo, instead of the status block.
//
// SSE is not used by codebases (read-only after create/delete), so
// the test surface is just the query + render path.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryRouter, RouterProvider } from "react-router";
import CodebasesPage from "../pages/codebases";
import CodebasePage from "../pages/codebase";

vi.mock("../../lib/api", () => ({
  api: {
    workspaces: {
      list: vi.fn(),
    },
    codebases: {
      list: vi.fn(),
      get: vi.fn(),
      create: vi.fn(),
      delete: vi.fn(),
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
];

const mockCodebases = [
  {
    id: "c1",
    workspace_id: "w1",
    path: "/home/u/repo",
    branch: "main",
    label: "Backend",
    created_at: "2026-06-01T00:00:00Z",
  },
  {
    id: "c2",
    workspace_id: "w1",
    path: "/home/u/mobile",
    branch: null,
    label: null,
    created_at: "2026-06-01T00:00:00Z",
  },
];

const happyDetail = {
  codebase: mockCodebases[0],
  git_status: {
    branch: "main",
    dirty_files: ["src/foo.ts"],
    recent_commits: [
      { hash: "abc1234567", message: "first commit" },
      { hash: "def4567890", message: "second commit" },
    ],
  },
  git_error: null,
};

const brokenDetail = {
  codebase: mockCodebases[0],
  git_status: null,
  git_error: "Path is not a git repository",
};

function renderList() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createMemoryRouter([{ path: "/codebases", element: <CodebasesPage /> }], {
    initialEntries: ["/codebases"],
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

function renderDetail(initialPath = "/workspaces/w1/codebases/c1") {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createMemoryRouter(
    [{ path: "/workspaces/:wid/codebases/:cid", element: <CodebasePage /> }],
    { initialEntries: [initialPath] },
  );
  return render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  // window.confirm is called by the delete button; default to true so
  // the test of the happy-path render is unaffected.
  vi.spyOn(window, "confirm").mockReturnValue(true);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("codebases list", () => {
  it("renders workspace header and a row per codebase", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: mockWorkspaces });
    mockApi.codebases.list.mockResolvedValue(mockCodebases);

    renderList();

    // Workspace header + 2 codebase rows. Use waitFor to handle the
    // async query resolution, then assert multiple text nodes (each
    // path appears in both the row label and the monospace caption).
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 3, name: "Default" })).toBeInTheDocument();
    });
    expect(screen.getByText("Backend")).toBeInTheDocument();
    // The second row has no label — falls back to the path (used twice:
    // once as the row label, once in the monospace caption).
    expect(screen.getAllByText("/home/u/mobile").length).toBeGreaterThan(0);
    // Paths are monospace — assert the monospace code path.
    expect(screen.getAllByText("/home/u/repo").length).toBeGreaterThan(0);
  });

  it("renders the empty-workspaces empty state when no workspaces exist", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: [] });
    renderList();
    expect(await screen.findByText("No workspaces")).toBeInTheDocument();
    expect(screen.getByText(/create a workspace first to register codebases/i)).toBeInTheDocument();
  });

  it("does not render a workspace section when its codebase list is empty", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: mockWorkspaces });
    mockApi.codebases.list.mockResolvedValue([]);

    renderList();
    // Wait for the workspaces query to resolve so the workspaces.map
    // runs; then assert that the per-workspace block is hidden (its
    // h3 header AND its row link are both absent). The create button
    // lives inside the same hidden block — confirm it's not there.
    await waitFor(() => {
      // The "All codebases across workspaces" subtitle is always
      // visible. Use it as a sentinel that the page finished loading.
      expect(
        screen.getByText(/git repositories registered across workspaces/i),
      ).toBeInTheDocument();
    });
    expect(screen.queryByRole("heading", { level: 3 })).toBeNull();
    expect(screen.queryByRole("button", { name: /\+ New codebase in Default/i })).toBeNull();
  });

  it("create button is disabled when path is empty", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: mockWorkspaces });
    mockApi.codebases.list.mockResolvedValue(mockCodebases);
    renderList();
    const button = await screen.findByRole("button", { name: /\+ New codebase in Default/i });
    fireEvent.click(button);
    const createButton = await screen.findByRole("button", { name: /create codebase/i });
    expect(createButton).toBeDisabled();
  });
});

describe("codebase detail (composite)", () => {
  it("renders the row, branch, dirty files, and recent commits", async () => {
    mockApi.codebases.get.mockResolvedValue(happyDetail);
    renderDetail();

    // Use waitFor to handle the async query resolution; the page
    // shows a spinner until the composite response arrives. Assert
    // on the dl values, the dirty file, the recent commits, and the
    // hashed commit prefix.
    await waitFor(
      () => {
        expect(screen.getByText("src/foo.ts")).toBeInTheDocument();
      },
      { timeout: 3000 },
    );
    // The h1 uses the codebase label "Backend" — also appears in
    // the dl's "Label" row, so use getAllByText.
    expect(screen.getAllByText("Backend").length).toBeGreaterThan(0);
    // Path appears in multiple places (header + dl).
    expect(screen.getAllByText("/home/u/repo").length).toBeGreaterThan(0);
    // Branch chip — appears in BOTH the dl AND the git status block
    // (they render the same value from the composite response).
    expect(screen.getAllByText("main").length).toBeGreaterThan(0);
    // First commit message + hash slice.
    expect(screen.getByText("first commit")).toBeInTheDocument();
    expect(screen.getByText("abc1234")).toBeInTheDocument();
    // No error banner.
    expect(screen.queryByText("Path is not a git repository")).toBeNull();
  });

  it("renders the git_error banner when the path is not a repo", async () => {
    mockApi.codebases.get.mockResolvedValue(brokenDetail);
    renderDetail();
    expect(await screen.findByText("Path is not a git repository")).toBeInTheDocument();
    // The git status section is omitted; the recent commits section
    // would not be present either.
    expect(screen.queryByText(/recent commits/i)).toBeNull();
  });

  it("still renders the codebase row when git_error is set", async () => {
    // The whole point of the graceful-degrade shape: the row stays
    // visible so the user can edit or delete even if git is broken.
    mockApi.codebases.get.mockResolvedValue(brokenDetail);
    renderDetail();
    // Wait for the query to settle; the row path should still be on screen.
    await waitFor(() => {
      expect(screen.getAllByText("/home/u/repo").length).toBeGreaterThan(0);
    });
  });
});

// late import so the line above is clean
import { fireEvent } from "@testing-library/react";
