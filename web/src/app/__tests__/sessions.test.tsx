// Page-level tests for the Sessions list page (feat-061) + the
// 4-step NewSessionWizard (feat-053).
//
// Mirrors `codebases.test.tsx`: mock the `api` surface, mount the page
// with a real QueryClient + MemoryRouter, assert rendered content and
// the new `+ New Session` per-workspace flow.
//
// Page-level coverage (regression guards, unchanged from the legacy
// single-page modal):
//   1. "No workspaces" empty state.
//   2. Per-workspace `+ New Session` button visible even with zero sessions.
//   3. Existing session rows still render alongside the per-workspace button.
//
// Wizard coverage (feat-053 — replaces the 3-field modal with a 4-step
// wizard: Runtime / Role / Model / Workspace):
//   4. Step chrome renders on open (Step 1 of 4 + Next enabled when
//      a healthy provider is mocked).
//   5. Step 0 empty state: `healthy: false` for every provider →
//      "configure a provider" link + Next disabled.
//   6. Step 0 healthy filter excludes unhealthy providers.
//   7. Step 1 compat matrix: all specialists are accepted under the
//      current "all allowed" policy (regression guard for the
//      future-tightening one-liner in `runtime-matrix.ts`).
//   8. Step 2 pre-selects the first model; clears on provider change.
//   9. Submit error highlights the failing step
//      (`cwd_outside_codebase` → step 4 / `runtime_mode_incompatible`
//      → step 1).
//  10. End-to-end 4-step submit produces the right payload
//      (runtime_kind, mode, codebase_id) and navigates to
//      `/sessions/:id`.
//  11. Nested NewCodebaseModal still works on Step 3 (replaces the
//      two legacy nested-modal tests).
//
// No SSE is involved on this page, so no `EventSource` stub is needed.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryRouter, RouterProvider } from "react-router";
import SessionsPage from "../pages/sessions";

vi.mock("../../lib/api", async (importOriginal) => {
  // `importOriginal` lets us forward the real `ApiError` class so
  // the wizard's `err instanceof ApiError` check (in onError) can
  // extract the code from thrown fakes. Without this, the mock
  // factory would replace the module wholesale and the `ApiError`
  // reference would be `undefined`.
  const actual = await importOriginal<typeof import("../../lib/api")>();
  return {
    ...actual,
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
        // feat-053: Step 2 of the wizard calls `api.providers.models(id)`
        // when the user picks a provider. Default to []; tests that
        // exercise the model pre-select override with `mockResolvedValueOnce`.
        models: vi.fn(),
      },
      specialists: {
        list: vi.fn(),
      },
      codebases: {
        list: vi.fn(),
        create: vi.fn(),
      },
      // feat-053: Step 3 of the wizard queries the unbound-task
      // endpoint. Default to []; tests that exercise the task picker
      // override with `mockResolvedValueOnce`.
      tasks: {
        unbound: vi.fn(),
      },
    },
  };
});

import { api, ApiError } from "../../lib/api";
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

// feat-053: every provider row gets `healthy: true` by default so the
// Step 0 picker is non-empty. Tests that need the empty/unhealthy
// state override with `mockResolvedValueOnce` (see `unhealthy: false`
// variants below). `kind: "http"` matches the legacy schema and
// resolves to the `anthropic-api` runtime via
// `defaultRuntimeForProviderKind`.
const mockProviders = [
  {
    id: "p1",
    type: "anthropic",
    kind: "http",
    name: "Anthropic",
    default_model: "claude-sonnet-4-5",
    binary_path: null,
    args_json: null,
    env_json: null,
    permission_mode: null,
    healthy: true,
    created_at: "2026-06-01T00:00:00Z",
  },
  {
    id: "p2",
    type: "openai",
    kind: "http",
    name: "OpenAI",
    default_model: "gpt-5",
    binary_path: null,
    args_json: null,
    env_json: null,
    permission_mode: null,
    healthy: true,
    created_at: "2026-06-01T00:00:00Z",
  },
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
  // Default: `models(id)` returns an empty list. Tests that exercise
  // the Step 2 pre-select override with `mockResolvedValueOnce` to
  // return a single entry.
  mockApi.providers.models.mockResolvedValue([]);
  mockApi.specialists.list.mockResolvedValue(mockSpecialists);
  // Default: no codebases registered. Each test that exercises the
  // codebase picker can override with `mockApi.codebases.list.mockResolvedValueOnce(...)`.
  // `api.codebases.list` returns Codebase[] (apiFetch unwraps the envelope),
  // so the mock returns the array directly.
  mockApi.codebases.list.mockResolvedValue([]);
  // Default: no unbound tasks in the workspace.
  mockApi.tasks.unbound.mockResolvedValue([]);
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

  // -----------------------------------------------------------------------
  // NewSessionWizard (feat-053) — 4-step flow
  // -----------------------------------------------------------------------

  /// Helper: opens the wizard for the Backend workspace, returning
  /// the router handle. Default state is "Step 1 of 4 · Runtime".
  async function openWizard() {
    const { router } = renderList();
    const button = await screen.findByRole("button", {
      name: /\+ New Session in Backend/i,
    });
    fireEvent.click(button);
    return { router };
  }

  it("renders the 4-step chrome on open and enables Next when a healthy provider is mocked", async () => {
    await openWizard();
    // Step 1 of 4 chrome (the "Runtime" step is step 0 in code, 1 in UI).
    // The sub-line text is split across child spans, so use a regex
    // against the full string instead of an exact match.
    expect(await screen.findByText(/Step 1 of 4\s*·\s*Runtime/)).toBeInTheDocument();
    // The label is also visible — assert it via the field's
    // LABEL_CLASS-prefixed name.
    expect(screen.getByText("Runtime Tool")).toBeInTheDocument();
    // The Back button is disabled on step 0. Match by exact name
    // so we don't collide with the per-workspace "+ New Session in
    // Backend" button.
    const backButton = screen.getByRole("button", { name: "Back" });
    expect(backButton).toBeDisabled();
    // Next is disabled until a healthy provider is picked.
    const nextButton = screen.getByRole("button", { name: "Next" });
    expect(nextButton).toBeDisabled();
    // The provider select is present.
    const providerSelect = screen.getByDisplayValue("Select provider…") as HTMLSelectElement;
    expect(providerSelect).toBeInTheDocument();
    // Pick the first provider — both are `healthy: true` by default.
    fireEvent.change(providerSelect, { target: { value: "p1" } });
    // Next is now enabled.
    expect(screen.getByRole("button", { name: "Next" })).not.toBeDisabled();
  });

  it("Step 0 empty state: when every provider is unhealthy, shows a 'configure a provider' link and disables Next", async () => {
    // Override the providers list to all-unhealthy.
    const unhealthy = mockProviders.map((p) => ({ ...p, healthy: false }));
    mockApi.providers.list.mockResolvedValueOnce(unhealthy);
    await openWizard();
    // The empty-state copy replaces the select; assert by the link.
    expect(await screen.findByText(/No healthy providers/i)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /settings/i })).toBeInTheDocument();
    // Next stays disabled.
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
  });

  it("Step 0 healthy filter excludes unhealthy providers from the option list", async () => {
    // Mix one healthy + one unhealthy. The wizard renders the
    // unhealthy row with `disabled` on the <option> tag (the user
    // sees it but cannot pick it). We assert on the option's
    // `disabled` property to confirm the filter, not the visible
    // text.
    const mixed = [
      { ...mockProviders[0], healthy: true },
      { ...mockProviders[1], healthy: false },
    ];
    mockApi.providers.list.mockResolvedValueOnce(mixed);
    await openWizard();
    const providerSelect = (await screen.findByDisplayValue(
      "Select provider…",
    )) as HTMLSelectElement;
    const options = Array.from(providerSelect.querySelectorAll("option"));
    // 1 placeholder + 2 providers.
    expect(options).toHaveLength(3);
    // p1 is healthy → enabled; p2 is unhealthy → disabled.
    expect((options[1] as HTMLOptionElement).disabled).toBe(false);
    expect((options[2] as HTMLOptionElement).disabled).toBe(true);
  });

  it("Step 1 compat matrix: the current 'all allowed' policy is non-regressing for both runtimes", async () => {
    // The current matrix (see `web/src/lib/runtime-matrix.ts`)
    // accepts all profiles on all runtimes. The wizard's Step 1
    // picker should show all specialists regardless of the chosen
    // provider's runtime. Switching providers should not change
    // the specialist list.
    await openWizard();
    const providerSelect = screen.getByDisplayValue("Select provider…") as HTMLSelectElement;
    fireEvent.change(providerSelect, { target: { value: "p1" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    // Step 2: specialist picker. Assert both specialists are listed.
    const specialistSelect = (await screen.findByDisplayValue(
      "No specialist",
    )) as HTMLSelectElement;
    expect(specialistSelect.querySelectorAll("option")).toHaveLength(1 + mockSpecialists.length);
    // Go back, switch provider, advance again. List should be the
    // same (compat matrix is "all allowed" today).
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    fireEvent.change(screen.getByDisplayValue("Anthropic"), { target: { value: "p2" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByDisplayValue("No specialist")).toBeInTheDocument();
  });

  it("Step 2 pre-selects the first model on provider change and clears when the user picks a different provider", async () => {
    // First call to `models(p1)` returns two models; second call
    // (after the user changes provider to p2) returns a different
    // first model.
    const p1Models = [
      { id: "claude-sonnet-4-5", name: "Claude Sonnet 4.5", context_window: 200000 },
      { id: "claude-opus-4-1", name: "Claude Opus 4.1", context_window: 200000 },
    ];
    const p2Models = [{ id: "gpt-5", name: "GPT-5", context_window: 256000 }];
    mockApi.providers.models.mockImplementation(async (id: string) =>
      id === "p1" ? p1Models : p2Models,
    );

    await openWizard();
    // Step 0: pick provider. The pre-select effect runs in the
    // background — the model input is only visible after we
    // advance to Step 2.
    const providerSelect = screen.getByDisplayValue("Select provider…") as HTMLSelectElement;
    fireEvent.change(providerSelect, { target: { value: "p1" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" })); // → Step 1
    fireEvent.click(screen.getByRole("button", { name: "Next" })); // → Step 2
    // The model input is now visible and pre-selected.
    await waitFor(() => {
      const modelInput = screen.getByDisplayValue("claude-sonnet-4-5") as HTMLInputElement;
      expect(modelInput).toBeInTheDocument();
    });

    // Go back to Step 0 and switch the provider. The pre-select
    // flag resets in the provider-change effect, and the new
    // provider's first model pre-selects on the next Step 2
    // arrival.
    fireEvent.click(screen.getByRole("button", { name: "Previous step" })); // → Step 1
    fireEvent.click(screen.getByRole("button", { name: "Previous step" })); // → Step 0
    // Re-fetch the provider select — the previous reference was
    // captured on a now-unmounted element (Step 0 was unmounted
    // when we advanced).
    const providerSelect2 = (await screen.findByDisplayValue("Anthropic")) as HTMLSelectElement;
    fireEvent.change(providerSelect2, { target: { value: "p2" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" })); // → Step 1
    fireEvent.click(screen.getByRole("button", { name: "Next" })); // → Step 2
    await waitFor(() => {
      const modelInput = screen.getByDisplayValue("gpt-5") as HTMLInputElement;
      expect(modelInput).toBeInTheDocument();
    });
  });

  it("submit error `cwd_outside_codebase` jumps to Step 4 (Workspace); `runtime_mode_incompatible` jumps to Step 1 (Runtime)", async () => {
    // First submit: cwd_outside_codebase. The wizard's onError
    // uses `err instanceof ApiError` to extract the code, so the
    // test fakes have to be real ApiError instances (not plain
    // Error+code shims). The status (400) matches what the
    // server returns for these validation codes.
    const cwdErr = new ApiError(400, "cwd_outside_codebase", "cwd must be inside codebase");
    mockApi.sessions.create.mockRejectedValueOnce(cwdErr);

    await openWizard();
    // Walk to Step 4 quickly.
    const providerSelect = screen.getByDisplayValue("Select provider…") as HTMLSelectElement;
    fireEvent.change(providerSelect, { target: { value: "p1" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText(/Step 4 of 4/)).toBeInTheDocument();
    // Submit → 400 with cwd_outside_codebase.
    fireEvent.click(screen.getByRole("button", { name: /create session/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/cwd/i);
    // The wizard jumps back to step 4.
    expect(screen.getByText(/Step 4 of 4/)).toBeInTheDocument();

    // Now advance to Step 4 again and submit with a different
    // error code. The jump target is different (step 1).
    // (We don't need to walk back manually — the test exercises the
    // jump on the next submit, which goes through onError with the
    // new code.)
    const rtErr = new ApiError(
      400,
      "runtime_mode_incompatible",
      "runtime kind not compatible with provider",
    );
    mockApi.sessions.create.mockRejectedValueOnce(rtErr);
    // The user is on step 4 after the previous submit; re-submit.
    fireEvent.click(screen.getByRole("button", { name: /create session/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/runtime/i);
    // The wizard jumps to step 1 (Runtime).
    expect(screen.getByText(/Step 1 of 4/)).toBeInTheDocument();
  });

  it("end-to-end 4-step submit produces the right payload (runtime_kind, mode, codebase_id) and navigates to /sessions/:id", async () => {
    const newSession = {
      id: "new-session-id",
      workspace_id: "w2",
      provider_id: "p1",
      specialist_id: "dev-crafter",
      parent_session_id: null,
      status: "connecting",
      model: "claude-sonnet-4-5",
      cwd: "/home/u/repo-b",
      codebase_id: "cb2",
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-01T00:00:00Z",
    };
    mockApi.sessions.create.mockResolvedValueOnce(newSession);
    mockApi.providers.models.mockResolvedValueOnce([
      { id: "claude-sonnet-4-5", name: "Claude Sonnet 4.5", context_window: 200000 },
    ]);
    mockApi.codebases.list.mockResolvedValueOnce([
      {
        id: "cb2",
        workspace_id: "w2",
        path: "/home/u/repo-b",
        branch: null,
        label: "Repo B",
        created_at: "2026-06-01T00:00:00Z",
      },
    ]);

    const { router } = await openWizard();

    // Step 0: pick provider.
    const providerSelect = screen.getByDisplayValue("Select provider…") as HTMLSelectElement;
    fireEvent.change(providerSelect, { target: { value: "p1" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));

    // Step 1: pick specialist.
    const specialistSelect = (await screen.findByDisplayValue(
      "No specialist",
    )) as HTMLSelectElement;
    fireEvent.change(specialistSelect, { target: { value: "dev-crafter" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));

    // Step 2: model pre-selects. Advance.
    await screen.findByDisplayValue("claude-sonnet-4-5");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));

    // Step 3: pick a codebase.
    const codebaseSelect = (await screen.findByDisplayValue(
      "No codebase (operate in workspace root)",
    )) as HTMLSelectElement;
    fireEvent.change(codebaseSelect, { target: { value: "cb2" } });

    // Submit.
    fireEvent.click(screen.getByRole("button", { name: /create session/i }));

    await waitFor(() => {
      // `kind: "http"` → runtime "anthropic-api" + mode "native" per
      // `defaultRuntimeForProviderKind`.
      expect(mockApi.sessions.create).toHaveBeenCalledWith("w2", {
        provider_id: "p1",
        specialist_id: "dev-crafter",
        model: "claude-sonnet-4-5",
        codebase_id: "cb2",
        runtime_kind: "anthropic-api",
        mode: "native",
      });
    });
    await waitFor(() => {
      expect(router.state.location.pathname).toBe("/sessions/new-session-id");
    });
  });

  it("clicking 'Register a codebase' on Step 3 opens a nested NewCodebaseModal; submitting it auto-selects the new codebase", async () => {
    // First list call (when Step 3 mounts) returns empty; second
    // call (after the create mutation invalidates) returns the new
    // codebase.
    const newCodebase = {
      id: "cb-new",
      workspace_id: "w2",
      path: "/home/u/repo-new",
      branch: null,
      label: "Repo New",
      created_at: "2026-06-09T00:00:00Z",
    };
    mockApi.codebases.list.mockResolvedValueOnce([]).mockResolvedValueOnce([newCodebase]);
    mockApi.codebases.create.mockResolvedValueOnce(newCodebase);

    await openWizard();
    // Walk to Step 3.
    const providerSelect = screen.getByDisplayValue("Select provider…") as HTMLSelectElement;
    fireEvent.change(providerSelect, { target: { value: "p1" } });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    fireEvent.click(screen.getByRole("button", { name: "Next" }));

    // The Step 3 empty-state hint exposes the register button.
    const registerButton = await screen.findByRole("button", { name: /register a codebase/i });
    fireEvent.click(registerButton);

    // The nested NewCodebaseModal opens.
    expect(await screen.findByRole("heading", { name: "New Codebase" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "New Session" })).toBeInTheDocument();

    // Fill the path and submit.
    const pathInput = screen.getByPlaceholderText("/Users/me/projects/my-app") as HTMLInputElement;
    fireEvent.change(pathInput, { target: { value: newCodebase.path } });
    const createCodebaseButton = screen.getByRole("button", { name: /create codebase/i });
    fireEvent.click(createCodebaseButton);

    await waitFor(() => {
      expect(mockApi.codebases.create).toHaveBeenCalledWith("w2", {
        path: newCodebase.path,
      });
    });
    // The NewCodebaseModal is gone; the NewSessionWizard is still open.
    expect(screen.queryByRole("heading", { name: "New Codebase" })).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "New Session" })).toBeInTheDocument();
    // The Step 3 codebase dropdown is now populated and shows the
    // new codebase as selected.
    const codebaseSelect = (await screen.findByDisplayValue("Repo New")) as HTMLSelectElement;
    expect(codebaseSelect).toBeInTheDocument();
    expect(codebaseSelect.value).toBe("cb-new");
  });
});
