import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { JourneySidebar } from "../pages/session/journey-sidebar";

// Mock the API module — the sidebar reads journey + fileChanges
// through these methods. Mocked in beforeEach so each test gets a
// clean function.
vi.mock("../../lib/api", () => ({
  api: {
    traces: {
      journey: vi.fn(),
      fileChanges: vi.fn(),
    },
  },
}));

import { api } from "../../lib/api";
const mockApi = vi.mocked(api);

function renderSidebar(isOpen = true) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  const onToggle = vi.fn();
  const utils = render(
    <QueryClientProvider client={queryClient}>
      <JourneySidebar sessionId="s1" isOpen={isOpen} onToggle={onToggle} />
    </QueryClientProvider>,
  );
  return { ...utils, onToggle };
}

beforeEach(() => {
  vi.clearAllMocks();
  // jsdom doesn't provide navigator.clipboard. Stub it so the
  // copy-to-clipboard path executes; the call is what the test
  // asserts on, not the underlying browser behavior.
  Object.assign(navigator, {
    clipboard: {
      writeText: vi.fn().mockResolvedValue(undefined),
    },
  });
});

afterEach(() => {
  // Reset the clipboard mock between tests so the timers and
  // mock-state from one test don't leak into the next.
  vi.useRealTimers();
});

describe("journey-view", () => {
  it("renders the panel heading when open", async () => {
    mockApi.traces.journey.mockResolvedValue([]);
    mockApi.traces.fileChanges.mockResolvedValue([]);
    renderSidebar(true);
    expect(await screen.findByText("Journey")).toBeInTheDocument();
  });

  it("does not render the panel when closed", () => {
    mockApi.traces.journey.mockResolvedValue([]);
    mockApi.traces.fileChanges.mockResolvedValue([]);
    renderSidebar(false);
    // The rail is always rendered; the panel content (e.g. the
    // "Decisions & Errors" section label) is only mounted when open.
    expect(screen.queryByText("Decisions & Errors")).not.toBeInTheDocument();
  });

  it("renders decision and error events from the journey endpoint", async () => {
    mockApi.traces.journey.mockResolvedValue([
      {
        id: "t1",
        session_id: "s1",
        event_type: "decision",
        summary: "use Rust for the new service",
        data_json: JSON.stringify({ text: "Rust gives us memory safety without GC" }),
        timestamp: "2026-01-01T00:00:00Z",
      },
      {
        id: "t2",
        session_id: "s1",
        event_type: "error",
        summary: "compilation failed in middleware/pipeline.ts:42",
        data_json: null,
        timestamp: "2026-01-01T00:01:00Z",
      },
    ]);
    mockApi.traces.fileChanges.mockResolvedValue([]);
    renderSidebar(true);

    expect(await screen.findByText("use Rust for the new service")).toBeInTheDocument();
    expect(
      await screen.findByText("compilation failed in middleware/pipeline.ts:42"),
    ).toBeInTheDocument();
    // Both chips are present.
    expect(screen.getAllByText("Decision").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Error").length).toBeGreaterThan(0);
  });

  it("expands a decision node to reveal the full text", async () => {
    mockApi.traces.journey.mockResolvedValue([
      {
        id: "t1",
        session_id: "s1",
        event_type: "decision",
        summary: "short summary",
        data_json: JSON.stringify({ text: "the full reasoning behind the choice" }),
        timestamp: "2026-01-01T00:00:00Z",
      },
    ]);
    mockApi.traces.fileChanges.mockResolvedValue([]);
    renderSidebar(true);

    // The full text is hidden behind a max-height transition;
    // assert the text is in the DOM but not visible until clicked.
    const full = await screen.findByText("the full reasoning behind the choice");
    expect(full).toBeInTheDocument();
    // The parent div has `max-h-0 opacity-0` when collapsed.
    const collapseContainer = full.closest(".max-h-0, [class*='max-h-0']");
    expect(collapseContainer).not.toBeNull();

    // Click the decision card to expand.
    const card = screen.getByRole("button", { name: /short summary/ });
    fireEvent.click(card);

    // After expansion the container should be `max-h-[600px] opacity-100`.
    const expanded = full.closest(".max-h-\\[600px\\]");
    expect(expanded).not.toBeNull();
  });

  it("renders the empty state when there are no events", async () => {
    mockApi.traces.journey.mockResolvedValue([]);
    mockApi.traces.fileChanges.mockResolvedValue([]);
    renderSidebar(true);
    expect(await screen.findByText(/No decisions or errors yet/)).toBeInTheDocument();
    expect(await screen.findByText(/No files touched yet/)).toBeInTheDocument();
  });

  it("renders file rows with action chips and copies the path on click", async () => {
    mockApi.traces.journey.mockResolvedValue([]);
    mockApi.traces.fileChanges.mockResolvedValue([
      { path: "src/auth.ts", actions: ["read", "write"], count: 3 },
      { path: "src/old-auth.ts", actions: ["delete"], count: 1 },
    ]);
    renderSidebar(true);

    // Both file paths appear.
    expect(await screen.findByText("src/auth.ts")).toBeInTheDocument();
    expect(await screen.findByText("src/old-auth.ts")).toBeInTheDocument();

    // Action chips are rendered.
    expect(screen.getAllByText("read").length).toBeGreaterThan(0);
    expect(screen.getAllByText("write").length).toBeGreaterThan(0);
    expect(screen.getAllByText("delete").length).toBeGreaterThan(0);

    // Click the file path to trigger the copy. `act()` wraps the
    // event so the deferred `setTimeout(... 1200)` (the "Copied!"
    // tooltip) doesn't fire outside of React's test scheduler.
    const pathRow = screen.getByTitle("Copy src/auth.ts");
    await act(async () => {
      fireEvent.click(pathRow);
    });

    // Clipboard mock should have been called with the path.
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("src/auth.ts");

    // The "Copied!" tooltip is now in the DOM. We don't assert on
    // its CSS transition state (jsdom doesn't run real animations);
    // we assert that the success copy was attempted and the
    // clipboard API was called.
  });

  it("rail toggle button calls onToggle", async () => {
    mockApi.traces.journey.mockResolvedValue([]);
    mockApi.traces.fileChanges.mockResolvedValue([]);
    const { onToggle } = renderSidebar(false);
    const rail = screen.getByTitle("Toggle Journey sidebar");
    fireEvent.click(rail);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("panel close (×) button calls onToggle when the panel is open", async () => {
    mockApi.traces.journey.mockResolvedValue([]);
    mockApi.traces.fileChanges.mockResolvedValue([]);
    const { onToggle } = renderSidebar(true);
    const close = screen.getByTitle("Hide Journey sidebar");
    fireEvent.click(close);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });
});
