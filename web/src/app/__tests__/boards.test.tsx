// Page-level tests for the Boards list page (feat-063 follow-up; mirrors
// the empty-state fix in `codebases.test.tsx` and the canonical
// `sessions.test.tsx`).
//
// Mirrors the structure of `codebases.test.tsx`: mock the `api` surface,
// mount the page with a real QueryClient + MemoryRouter, assert rendered
// content. The test suite covers:
//
//   1. List page renders boards grouped by workspace and the
//      `+ New board in {name}` button per workspace.
//   2. The per-workspace section is always visible — heading AND
//      create button — even when the workspace has zero boards
//      (the empty-state fix; previously the whole block returned null).
//   3. Clicking the per-workspace button opens the NewBoardModal
//      pre-bound to that workspace; submitting it calls
//      `api.kanban.boards.create` with the right payload and navigates
//      to the new board's detail page.
//   4. Empty-state copy is shown when no workspaces exist.
//
// SSE is not used by the /boards list (read-only after create/delete),
// so the test surface is just the query + render path.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryRouter, RouterProvider } from "react-router";
import BoardsPage from "../pages/boards";

vi.mock("../../lib/api", () => ({
  api: {
    workspaces: {
      list: vi.fn(),
    },
    kanban: {
      boards: {
        list: vi.fn(),
        get: vi.fn(),
        create: vi.fn(),
        update: vi.fn(),
        delete: vi.fn(),
      },
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

const mockBoards = [
  {
    id: "b1",
    workspace_id: "w1",
    name: "Sprint 1",
    created_at: "2026-06-01T00:00:00Z",
  },
  {
    id: "b2",
    workspace_id: "w1",
    name: "Backlog",
    created_at: "2026-06-01T00:00:00Z",
  },
];

function renderList() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  // The router intentionally has no `/workspaces/:wid/boards/:bid` route —
  // the post-create navigate is asserted via `router.state.location.pathname`
  // only. The 404 the unknown route logs is expected and does not affect the
  // test outcome. (We can't add a catch-all here without breaking the
  // navigate call from the post-create onSuccess.)
  const router = createMemoryRouter([{ path: "/boards", element: <BoardsPage /> }], {
    initialEntries: ["/boards"],
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
  // Silence the React Router 404 that the unknown detail route logs during
  // the create-and-navigate flow. The 404 is expected (we deliberately do
  // not mount the detail page) and does not affect the test outcome.
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("boards list", () => {
  it("renders workspace header and a row per board", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: mockWorkspaces });
    mockApi.kanban.boards.list.mockResolvedValue(mockBoards);

    renderList();

    // Workspace header + 2 board rows.
    await waitFor(() => {
      expect(screen.getByRole("heading", { level: 3, name: "Default" })).toBeInTheDocument();
    });
    expect(screen.getByText("Sprint 1")).toBeInTheDocument();
    expect(screen.getByText("Backlog")).toBeInTheDocument();
  });

  it("renders the empty-workspaces empty state when no workspaces exist", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: [] });
    renderList();
    expect(await screen.findByText("No workspaces")).toBeInTheDocument();
    expect(screen.getByText(/create a workspace first to start boards/i)).toBeInTheDocument();
  });

  it("renders the workspace heading and + New board button even when the board list is empty", async () => {
    // The pre-fix behavior was: `WorkspaceBoards` returned null when
    // `boards.length === 0`, leaving the page with no entry point to
    // register the first board. The fix (mirroring feat-061 in
    // sessions.tsx / feat-063 in codebases.tsx) keeps the heading +
    // button visible and renders an inline "No boards yet" placeholder.
    mockApi.workspaces.list.mockResolvedValue({ data: mockWorkspaces });
    mockApi.kanban.boards.list.mockResolvedValue([]);

    renderList();

    // The workspace heading and the per-workspace create button are
    // both present, so the user can still register a board.
    expect(await screen.findByRole("heading", { level: 3, name: "Default" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /\+ New board in Default/i })).toBeInTheDocument();
    // The inline empty-state copy is rendered (in place of the list).
    expect(screen.getByText(/no boards yet/i)).toBeInTheDocument();
  });

  it("clicking the per-workspace button opens the NewBoardModal", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: mockWorkspaces });
    mockApi.kanban.boards.list.mockResolvedValue(mockBoards);

    renderList();
    const trigger = await screen.findByRole("button", {
      name: /\+ New board in Default/i,
    });
    fireEvent.click(trigger);

    // The modal header appears.
    expect(await screen.findByRole("heading", { name: "New Board" })).toBeInTheDocument();
    // The name input is present.
    const nameInput = screen.getByPlaceholderText(/e\.g\. Product Sprint Q3/i);
    expect(nameInput).toBeInTheDocument();
  });

  it("submitting the form creates a board and navigates to /workspaces/:wid/boards/:bid", async () => {
    const newBoard = {
      id: "new-bid",
      workspace_id: "w1",
      name: "Sprint 2",
      created_at: "2026-06-01T00:00:00Z",
    };
    mockApi.workspaces.list.mockResolvedValue({ data: mockWorkspaces });
    mockApi.kanban.boards.list.mockResolvedValue(mockBoards);
    mockApi.kanban.boards.create.mockResolvedValueOnce(newBoard);

    const { router } = renderList();

    const trigger = await screen.findByRole("button", {
      name: /\+ New board in Default/i,
    });
    fireEvent.click(trigger);

    // Fill the form and submit. The modal now defaults to the
    // "Standard" template (5 columns + bundled specialists +
    // stage mapping), so the create payload carries `columns` too.
    const nameInput = (await screen.findByPlaceholderText(
      /e\.g\. Product Sprint Q3/i,
    )) as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: "Sprint 2" } });

    const submit = screen.getByRole("button", { name: /create board/i });
    fireEvent.click(submit);

    await waitFor(() => {
      expect(mockApi.kanban.boards.create).toHaveBeenCalledWith("w1", {
        name: "Sprint 2",
        columns: expect.arrayContaining([
          expect.objectContaining({ name: "Backlog", stage: "backlog" }),
          expect.objectContaining({ name: "To Do", stage: "todo" }),
          expect.objectContaining({ name: "In Progress", stage: "dev" }),
          expect.objectContaining({ name: "Review", stage: "review" }),
          expect.objectContaining({ name: "Done", stage: "done" }),
        ]),
      });
    });
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/workspaces/w1/boards/new-bid");
    });
  });

  it("the create button is disabled when the name is empty", async () => {
    mockApi.workspaces.list.mockResolvedValue({ data: mockWorkspaces });
    mockApi.kanban.boards.list.mockResolvedValue(mockBoards);
    renderList();
    const trigger = await screen.findByRole("button", { name: /\+ New board in Default/i });
    fireEvent.click(trigger);
    const createButton = await screen.findByRole("button", { name: /create board/i });
    expect(createButton).toBeDisabled();
  });
});
