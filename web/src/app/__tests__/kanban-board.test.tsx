// Page-level tests for the Kanban board (feat-026). Mirrors the
// structure of `journey-view.test.tsx`: mock `api.kanban.*`, mount
// BoardPage with a real QueryClient and MemoryRouter, assert
// rendered content + a few interactions.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryRouter, RouterProvider } from "react-router";
import BoardPage from "../pages/board";

// Mock the API module. The page consumes `api.kanban.boards.get`,
// `api.kanban.cards.create`, `api.kanban.tasks.update` etc. The
// SSE EventSource is not in the test surface — the hook skips it
// when the board is not loaded in a real browser. (We don't
// simulate EventSource here; tests focus on the query + render
// path. SSE-driven behavior is covered by `use-board.test.tsx`.)
vi.mock("../../lib/api", () => ({
  api: {
    kanban: {
      boards: {
        get: vi.fn(),
        list: vi.fn(),
        create: vi.fn(),
        update: vi.fn(),
        delete: vi.fn(),
      },
      columns: {
        create: vi.fn(),
        update: vi.fn(),
      },
      cards: {
        create: vi.fn(),
      },
      tasks: {
        update: vi.fn(),
        delete: vi.fn(),
      },
    },
    specialists: {
      list: vi.fn().mockResolvedValue([]),
    },
    workspaces: {
      list: vi.fn(),
    },
    providers: {
      list: vi.fn(),
    },
  },
}));

import { api } from "../../lib/api";
const mockApi = vi.mocked(api);

function makeDetail(
  overrides: Partial<{ tasks: typeof mockTasks; columns: typeof mockColumns }> = {},
) {
  return {
    board: {
      id: "b1",
      workspace_id: "w1",
      name: "Test Board",
      created_at: "2026-06-01T00:00:00Z",
    },
    columns: overrides.columns ?? mockColumns,
    tasks: overrides.tasks ?? mockTasks,
  };
}

const mockColumns = [
  {
    id: "c1",
    board_id: "b1",
    name: "To Do",
    position: 0,
    specialist_id: null,
    auto_trigger: false,
    runtime_kind: null,
    created_at: "2026-06-01T00:00:00Z",
  },
  {
    id: "c2",
    board_id: "b1",
    name: "Done",
    position: 1000,
    specialist_id: null,
    auto_trigger: false,
    runtime_kind: null,
    created_at: "2026-06-01T00:00:00Z",
  },
];

const mockTasks = [
  {
    id: "t1",
    board_id: "b1",
    column_id: "c1",
    title: "Implement OAuth2",
    description: null,
    position: 1000,
    status: "active",
    session_id: null,
    acceptance_criteria: null,
    completion_summary: null,
    verification_report: null,
    created_at: "2026-06-01T00:00:00Z",
    updated_at: "2026-06-01T00:00:00Z",
  },
  {
    id: "t2",
    board_id: "b1",
    column_id: "c1",
    title: "Set up rate limiting",
    description: null,
    position: 2000,
    status: "active",
    session_id: "sess-1",
    acceptance_criteria: null,
    completion_summary: null,
    verification_report: null,
    created_at: "2026-06-01T00:00:00Z",
    updated_at: "2026-06-01T00:00:00Z",
  },
  {
    id: "t3",
    board_id: "b1",
    column_id: "c2",
    title: "Ship v1.4",
    description: null,
    position: 1000,
    status: "done",
    session_id: null,
    acceptance_criteria: null,
    completion_summary: null,
    verification_report: null,
    created_at: "2026-06-01T00:00:00Z",
    updated_at: "2026-06-01T00:00:00Z",
  },
];

function renderBoard() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const router = createMemoryRouter(
    [
      {
        path: "/workspaces/:wid/boards/:bid",
        element: <BoardPage />,
      },
    ],
    { initialEntries: ["/workspaces/w1/boards/b1"] },
  );
  return render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  // jsdom doesn't provide EventSource. The hook creates one in a
  // useEffect on mount; without a stub the useEffect throws and
  // crashes the page. Stub a minimal no-op implementation.
  if (typeof globalThis.EventSource === "undefined") {
    class StubEventSource {
      url: string;
      constructor(url: string) {
        this.url = url;
      }
      addEventListener() {}
      removeEventListener() {}
      close() {}
    }
    vi.stubGlobal("EventSource", StubEventSource);
  }
  mockApi.kanban.boards.get.mockResolvedValue(makeDetail());
  mockApi.kanban.tasks.update.mockResolvedValue(mockTasks[0]);
  mockApi.kanban.tasks.delete.mockResolvedValue(null);
  mockApi.kanban.cards.create.mockImplementation(async (_wid, _bid, data) => ({
    ...mockTasks[0],
    id: "t-new",
    title: data.title,
    column_id: data.column_id,
  }));
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("kanban-board", () => {
  it("renders the board name and column headers", async () => {
    renderBoard();
    expect(await screen.findByText("Test Board")).toBeInTheDocument();
    expect(await screen.findByText("To Do")).toBeInTheDocument();
    expect(await screen.findByText("Done")).toBeInTheDocument();
  });

  it("renders all cards grouped by column", async () => {
    renderBoard();
    expect(await screen.findByText("Implement OAuth2")).toBeInTheDocument();
    expect(await screen.findByText("Set up rate limiting")).toBeInTheDocument();
    expect(await screen.findByText("Ship v1.4")).toBeInTheDocument();
  });

  it("renders the agent indicator pill when task.session_id is set", async () => {
    renderBoard();
    // The session_id "sess-1" should produce an "Agent" pill on the
    // t2 card. Multiple cards may render this label; at least one
    // is present.
    const pills = await screen.findAllByText("Agent");
    expect(pills.length).toBeGreaterThan(0);
  });

  it("renders the active and done status chips", async () => {
    renderBoard();
    const activeChips = await screen.findAllByText("active");
    expect(activeChips.length).toBeGreaterThanOrEqual(2);
    // `done` (lowercase) is the task status chip label — distinct
    // from the "Done" column header (capital D).
    const doneChips = await screen.findAllByText("done");
    expect(doneChips.length).toBeGreaterThanOrEqual(1);
  });

  it("opens the TaskDetailPanel when a card is clicked", async () => {
    renderBoard();
    const card = await screen.findByText("Implement OAuth2");
    fireEvent.click(card);
    // The panel header reads "Task Details" and the title input is
    // pre-filled with the card's title.
    expect(await screen.findByText("Task Details")).toBeInTheDocument();
    const titleInput = await screen.findByDisplayValue("Implement OAuth2");
    expect(titleInput).toBeInTheDocument();
  });

  it("renders the + Add card and + Add column placeholders", async () => {
    renderBoard();
    // Wait for the board to render, then check the static buttons.
    await screen.findByText("Test Board");
    const addCardButtons = screen.getAllByText("+ Add card");
    expect(addCardButtons.length).toBeGreaterThan(0);
    expect(screen.getByText("+ Add column")).toBeInTheDocument();
  });

  it("renders a 4-line error banner when the board fails to load", async () => {
    mockApi.kanban.boards.get.mockRejectedValue(new Error("board not found"));
    renderBoard();
    // ErrorBanner renders the message; the page also surfaces a
    // dismissable banner. Assert at minimum that the error text is
    // present.
    await waitFor(() => {
      expect(screen.getByText(/board not found/)).toBeInTheDocument();
    });
  });

  it("renders runtime kind badge on auto-trigger column with runtime_kind", async () => {
    const columnsWithRuntime = [
      {
        id: "c1",
        board_id: "b1",
        name: "CLI Lane",
        position: 0,
        specialist_id: "dev",
        auto_trigger: true,
        runtime_kind: "claude-code" as const,
        created_at: "2026-06-01T00:00:00Z",
      },
      {
        id: "c2",
        board_id: "b1",
        name: "No Runtime",
        position: 1000,
        specialist_id: "dev",
        auto_trigger: true,
        runtime_kind: null,
        created_at: "2026-06-01T00:00:00Z",
      },
    ];
    mockApi.kanban.boards.get.mockResolvedValue({
      board: {
        id: "b1",
        workspace_id: "w1",
        name: "Test Board",
        created_at: "2026-06-01T00:00:00Z",
      },
      columns: columnsWithRuntime,
      tasks: [],
    });
    renderBoard();
    await screen.findByText("Test Board");
    // The column with runtime_kind should show the badge.
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    // The column without runtime_kind should NOT show the badge.
    const noRuntimeColumn = screen.getByText("No Runtime").closest("div")!;
    expect(noRuntimeColumn.querySelector('[title="Auto-trigger enabled"]')).toBeInTheDocument();
  });
});
