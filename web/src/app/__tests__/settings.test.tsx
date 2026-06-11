import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
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
const mockApi = vi.mocked(api, true);

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

// Complete Provider shape (post-feat-039). All test fixtures include `kind`
// so they type-check against the widened `Provider` interface.
const baseProvider = {
  type: "anthropic",
  name: "Production",
  default_model: null,
  binary_path: null,
  args_json: null,
  env_json: null,
  permission_mode: null,
  created_at: "2025-01-15T00:00:00Z",
} as const;

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
    // Kind-agnostic copy (feat-052): the previous "Add an Anthropic
    // provider to get started" string was anchored to the pre-kind
    // single-form era.
    expect(screen.getByText("Add a provider to get started")).toBeInTheDocument();
  });

  it("renders provider list with kind badge", async () => {
    mockApi.providers.list.mockResolvedValue([
      { id: "p-1", kind: "http", ...baseProvider },
      {
        id: "p-2",
        kind: "cli",
        type: "claude-code",
        name: "Codex CLI",
        default_model: null,
        binary_path: "/usr/local/bin/codex",
        args_json: null,
        env_json: null,
        permission_mode: "default",
        created_at: "2025-01-16T00:00:00Z",
      },
    ]);
    renderSettings();
    // Both provider names render.
    expect(await screen.findByText("Production")).toBeInTheDocument();
    expect(screen.getByText("Codex CLI")).toBeInTheDocument();
    // Kind badges render in the name column, uppercase per the styling.
    const httpBadge = screen.getAllByText("http");
    const cliBadge = screen.getAllByText("cli");
    expect(httpBadge.length).toBeGreaterThan(0);
    expect(cliBadge.length).toBeGreaterThan(0);
  });

  it("shows the + Add Provider button (replaces the inlined form)", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    // The Add Provider button is the trigger that opens the kind-aware
    // modal; the form itself is no longer inlined at page level.
    const addButton = await screen.findByRole("button", { name: /add provider/i });
    expect(addButton).toBeInTheDocument();
  });

  it("clicking + Add Provider opens the kind-aware modal with HTTP selected by default", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    const addButton = await screen.findByRole("button", { name: /add provider/i });
    fireEvent.click(addButton);
    // Modal header.
    expect(await screen.findByRole("heading", { name: "Add Provider" })).toBeInTheDocument();
    // HTTP is the default kind (pre-selected).
    const httpRadio = screen.getByRole("radio", { name: "HTTP" }) as HTMLButtonElement;
    expect(httpRadio.getAttribute("aria-checked")).toBe("true");
    // HTTP fields visible by default.
    expect(screen.getByPlaceholderText("e.g. Production")).toBeInTheDocument();
    expect(screen.getByPlaceholderText("sk-ant-...")).toBeInTheDocument();
    // CLI fields hidden.
    expect(screen.queryByPlaceholderText("/usr/local/bin/claude")).not.toBeInTheDocument();
  });

  it("switching to CLI shows the binary_path + args + env + permission_mode fields", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    fireEvent.click(await screen.findByRole("button", { name: /add provider/i }));
    // Click the CLI radio.
    const cliRadio = await screen.findByRole("radio", { name: "CLI" });
    fireEvent.click(cliRadio);
    // CLI-only fields are now visible.
    expect(screen.getByPlaceholderText("/usr/local/bin/claude")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /add arg/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /add env var/i })).toBeInTheDocument();
    // Permission mode dropdown is present and has 4 options + 1
    // placeholder. The select is queried by its initial display
    // value (the "Select…" placeholder) since the label is not
    // programmatically associated with the select.
    const permSelect = screen.getByDisplayValue("Select…") as HTMLSelectElement;
    expect(permSelect).toBeInTheDocument();
    expect(permSelect.querySelectorAll("option")).toHaveLength(1 + 4);
    // HTTP fields are hidden.
    expect(screen.queryByPlaceholderText("sk-ant-...")).not.toBeInTheDocument();
  });

  it("submitting the HTTP form sends kind=http with the right payload", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    mockApi.providers.create.mockResolvedValue({
      id: "new-id",
      kind: "http",
      type: "anthropic",
      name: "Test HTTP",
      default_model: "claude-sonnet-4-20250514",
      binary_path: null,
      args_json: null,
      env_json: null,
      permission_mode: null,
      created_at: "2025-01-15T00:00:00Z",
    });
    renderSettings();
    fireEvent.click(await screen.findByRole("button", { name: /add provider/i }));
    // Fill in name + api key (base_url pre-fills with the Anthropic default).
    fireEvent.change(await screen.findByPlaceholderText("e.g. Production"), {
      target: { value: "Test HTTP" },
    });
    fireEvent.change(screen.getByPlaceholderText("sk-ant-..."), {
      target: { value: "sk-ant-test-key" },
    });
    // Submit.
    fireEvent.click(screen.getByRole("button", { name: /save provider/i }));
    await waitFor(() => {
      expect(mockApi.providers.create).toHaveBeenCalledWith(
        expect.objectContaining({
          kind: "http",
          type: "anthropic",
          name: "Test HTTP",
          base_url: "https://api.anthropic.com",
          api_key: "sk-ant-test-key",
        }),
      );
    });
  });

  it("submitting the CLI form sends kind=cli with args + env + permission_mode", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    mockApi.providers.create.mockResolvedValue({
      id: "new-id",
      kind: "cli",
      type: "claude-code",
      name: "Test CLI",
      default_model: null,
      binary_path: "/usr/local/bin/claude",
      args_json: '["-p","--verbose"]',
      env_json: '{"DEBUG":"1"}',
      permission_mode: "default",
      created_at: "2025-01-15T00:00:00Z",
    });
    renderSettings();
    fireEvent.click(await screen.findByRole("button", { name: /add provider/i }));
    // Switch to CLI.
    fireEvent.click(await screen.findByRole("radio", { name: "CLI" }));
    // Fill in name + binary_path.
    fireEvent.change(screen.getByPlaceholderText("e.g. Production"), {
      target: { value: "Test CLI" },
    });
    fireEvent.change(screen.getByPlaceholderText("/usr/local/bin/claude"), {
      target: { value: "/usr/local/bin/claude" },
    });
    // Add an arg.
    fireEvent.click(screen.getByRole("button", { name: /add arg/i }));
    const argInputs = screen.getAllByPlaceholderText(/arg \d/);
    fireEvent.change(argInputs[0], { target: { value: "-p" } });
    fireEvent.click(screen.getByRole("button", { name: /add arg/i }));
    const argInputsAfter = screen.getAllByPlaceholderText(/arg \d/);
    fireEvent.change(argInputsAfter[1], { target: { value: "--verbose" } });
    // Add an env var.
    fireEvent.click(screen.getByRole("button", { name: /add env var/i }));
    const envKey = screen.getByPlaceholderText("KEY");
    const envVal = screen.getByPlaceholderText("value");
    fireEvent.change(envKey, { target: { value: "DEBUG" } });
    fireEvent.change(envVal, { target: { value: "1" } });
    // Pick a permission mode (queried by its placeholder display value;
    // the label is not programmatically associated with the select).
    const permSelect = screen.getByDisplayValue("Select…") as HTMLSelectElement;
    fireEvent.change(permSelect, { target: { value: "default" } });
    // Submit.
    fireEvent.click(screen.getByRole("button", { name: /save provider/i }));
    await waitFor(() => {
      expect(mockApi.providers.create).toHaveBeenCalledWith(
        expect.objectContaining({
          kind: "cli",
          type: "claude-code",
          name: "Test CLI",
          binary_path: "/usr/local/bin/claude",
          args_json: '["-p","--verbose"]',
          env_json: '{"DEBUG":"1"}',
          permission_mode: "default",
        }),
      );
    });
  });

  it("switching kind back to HTTP hides the CLI fields again", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    fireEvent.click(await screen.findByRole("button", { name: /add provider/i }));
    // CLI on.
    fireEvent.click(await screen.findByRole("radio", { name: "CLI" }));
    expect(screen.getByPlaceholderText("/usr/local/bin/claude")).toBeInTheDocument();
    // Back to HTTP.
    fireEvent.click(screen.getByRole("radio", { name: "HTTP" }));
    expect(screen.queryByPlaceholderText("/usr/local/bin/claude")).not.toBeInTheDocument();
    expect(screen.getByPlaceholderText("sk-ant-...")).toBeInTheDocument();
  });

  it("name and default_model are preserved across a kind switch", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    fireEvent.click(await screen.findByRole("button", { name: /add provider/i }));
    // Fill in name + default_model on HTTP.
    fireEvent.change(await screen.findByPlaceholderText("e.g. Production"), {
      target: { value: "Shared Name" },
    });
    const modelInput = screen.getByDisplayValue("claude-sonnet-4-20250514");
    fireEvent.change(modelInput, { target: { value: "claude-opus-4-20250514" } });
    // Switch to CLI.
    fireEvent.click(screen.getByRole("radio", { name: "CLI" }));
    // Name preserved.
    const nameAfter = screen.getByPlaceholderText("e.g. Production") as HTMLInputElement;
    expect(nameAfter.value).toBe("Shared Name");
    // default_model preserved (the CLI form has its own model input that
    // is also controlled by the same state — find it via the same
    // value we set, which is now in the CLI form).
    const modelAfter = screen.getByDisplayValue("claude-opus-4-20250514");
    expect(modelAfter).toBeInTheDocument();
  });

  it("submitting the empty form shows an inline Name-required error", async () => {
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    fireEvent.click(await screen.findByRole("button", { name: /add provider/i }));
    // The form has the Anthropic default base_url pre-filled but no api_key
    // and no name; submitting should surface an inline error and not
    // call the create endpoint.
    fireEvent.click(await screen.findByRole("button", { name: /save provider/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Name is required");
    expect(mockApi.providers.create).not.toHaveBeenCalled();
  });

  it("clearing the default_model field shows an inline Default-model-required error", async () => {
    // The Rust handler at api/providers.rs:107-119 requires default_model
    // for both kinds. The modal pre-fills a sensible default but lets
    // the user clear it; client-side validation must surface the error
    // before the call, so the user does not see a raw 400.
    mockApi.providers.list.mockResolvedValue([]);
    renderSettings();
    fireEvent.click(await screen.findByRole("button", { name: /add provider/i }));
    // Fill name + api_key (the pre-filled default_model is the
    // anthropic default — clear it).
    fireEvent.change(await screen.findByPlaceholderText("e.g. Production"), {
      target: { value: "Test HTTP" },
    });
    fireEvent.change(screen.getByPlaceholderText("sk-ant-..."), {
      target: { value: "sk-ant-test-key" },
    });
    fireEvent.change(screen.getByDisplayValue("claude-sonnet-4-20250514"), {
      target: { value: "" },
    });
    fireEvent.click(screen.getByRole("button", { name: /save provider/i }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Default model is required");
    expect(mockApi.providers.create).not.toHaveBeenCalled();
  });
});
