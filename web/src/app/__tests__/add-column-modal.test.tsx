// Page-level test for the Add Column modal's stage + runtime +
// specialist-description fields (feat-068 F-2, F-3, F-5, F-7).
// Mirrors the structure of `kanban-board.test.tsx`: mock the
// API surface, mount the modal directly, assert the form state
// and the POST payload shape.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { AddColumnModal } from "../pages/board/add-column-modal";

vi.mock("../../lib/api", () => ({
  api: {
    specialists: {
      list: vi.fn(),
    },
  },
}));

import { api } from "../../lib/api";
const mockApi = vi.mocked(api);

const mockSpecialists = [
  {
    name: "todo-orchestrator",
    description: "Validates stories before the developer picks them up",
    model: "claude-sonnet-4-6",
    tool_profile: "planning",
    tags: ["backlog", "refinement"],
  },
  {
    name: "dev-crafter",
    description: "Implements the change in the codebase",
    model: "claude-sonnet-4-6",
    tool_profile: "implementation",
    tags: ["development"],
  },
];

function renderModal(onSubmit: ReturnType<typeof vi.fn>) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={qc}>
      <AddColumnModal open onClose={() => {}} onSubmit={onSubmit} isSubmitting={false} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockApi.specialists.list.mockResolvedValue(mockSpecialists);
});

describe("add-column-modal (feat-068)", () => {
  it("renders the stage radio with the 5 canonical values", () => {
    const onSubmit = vi.fn();
    renderModal(onSubmit);
    // The Stage radios are <button role="radio"> elements; the
    // accessible name is the button's text content.
    expect(screen.getByRole("radio", { name: "Backlog" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "To Do" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "In Progress" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Review" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Done" })).toBeInTheDocument();
    // Default selection: todo (the most common auto-trigger intent).
    expect(screen.getByRole("radio", { name: "To Do" })).toHaveAttribute("aria-checked", "true");
  });

  it("renders the runtime_kind dropdown with Inherit as the default", () => {
    const onSubmit = vi.fn();
    renderModal(onSubmit);
    const select = screen.getByLabelText("Runtime Tool") as HTMLSelectElement;
    expect(select.value).toBe("inherit");
    // The 5 options: Inherit, Anthropic API, Claude Code, Codex, OpenCode.
    const labels = Array.from(select.options).map((o) => o.text);
    expect(labels).toEqual([
      "Inherit from provider",
      "Anthropic API",
      "Claude Code (CLI)",
      "Codex (CLI)",
      "OpenCode (CLI)",
    ]);
  });

  it("sends the chosen stage and runtime_kind in the onSubmit payload", async () => {
    const onSubmit = vi.fn();
    renderModal(onSubmit);

    // Set name.
    fireEvent.change(screen.getByPlaceholderText(/e\.g\. In Review/i), {
      target: { value: "Review" },
    });
    // Pick stage = review.
    fireEvent.click(screen.getByRole("radio", { name: "Review" }));
    // Pick runtime = claude-code.
    fireEvent.change(screen.getByLabelText("Runtime Tool"), {
      target: { value: "claude-code" },
    });
    // Enable auto-trigger so the specialist select becomes required.
    fireEvent.click(screen.getByRole("checkbox"));
    // Wait for the specialist list to populate.
    const specialistSelect = await waitFor(() => {
      const el = screen.getByLabelText("Specialist *") as HTMLSelectElement;
      if (el.options.length < 3) throw new Error("specialists not loaded");
      return el;
    });
    fireEvent.change(specialistSelect, {
      target: { value: "dev-crafter" },
    });

    fireEvent.click(screen.getByRole("button", { name: "Add column" }));

    expect(onSubmit).toHaveBeenCalledWith({
      name: "Review",
      auto_trigger: true,
      specialist_id: "dev-crafter",
      stage: "review",
      runtime_kind: "claude-code",
    });
  });

  it("omits runtime_kind when Inherit is selected", () => {
    const onSubmit = vi.fn();
    renderModal(onSubmit);
    fireEvent.change(screen.getByPlaceholderText(/e\.g\. In Review/i), {
      target: { value: "Backlog" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add column" }));
    expect(onSubmit).toHaveBeenCalledWith({
      name: "Backlog",
      auto_trigger: undefined,
      specialist_id: undefined,
      stage: "todo",
      runtime_kind: undefined,
    });
  });

  it("shows the specialist description inline in the dropdown options (F-7)", async () => {
    const onSubmit = vi.fn();
    renderModal(onSubmit);
    // Enable auto-trigger so the specialist select becomes visible.
    fireEvent.click(screen.getByRole("checkbox"));
    const select = await waitFor(() => {
      const el = screen.getByLabelText("Specialist *") as HTMLSelectElement;
      if (el.options.length < 3) throw new Error("specialists not loaded");
      return el;
    });
    const optionTexts = Array.from(select.options).map((o) => o.text);
    expect(optionTexts).toContain(
      "todo-orchestrator — Validates stories before the developer picks them up",
    );
    expect(optionTexts).toContain("dev-crafter — Implements the change in the codebase");
  });
});
