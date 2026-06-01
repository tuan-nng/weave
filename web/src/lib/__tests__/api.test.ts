import { describe, it, expect, vi, beforeEach } from "vitest";
import { api, ApiError } from "../api";

// Mock fetch globally
const mockFetch = vi.fn();
vi.stubGlobal("fetch", mockFetch);

beforeEach(() => {
  mockFetch.mockReset();
});

describe("ApiError", () => {
  it("has status, code, and message", () => {
    const err = new ApiError(400, "validation_error", "bad input");
    expect(err.status).toBe(400);
    expect(err.code).toBe("validation_error");
    expect(err.message).toBe("bad input");
    expect(err.name).toBe("ApiError");
  });
});

describe("api.health", () => {
  it("returns flat health response (no data envelope)", async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: () => Promise.resolve({ status: "ok", version: "0.1.0", uptime_seconds: 42 }),
    });

    const result = await api.health();
    expect(result.status).toBe("ok");
    expect(result.version).toBe("0.1.0");
  });
});

describe("api.workspaces", () => {
  it("unwraps {data: T} envelope on success", async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: () =>
        Promise.resolve({
          data: { id: "ws-1", name: "default", status: "active", created_at: "", updated_at: "" },
        }),
    });

    const ws = await api.workspaces.get("ws-1");
    expect(ws.id).toBe("ws-1");
    expect(ws.name).toBe("default");
  });

  it("throws ApiError on non-ok response", async () => {
    mockFetch.mockResolvedValue({
      ok: false,
      status: 404,
      json: () => Promise.resolve({ error: { code: "not_found", message: "Workspace not found" } }),
    });

    await expect(api.workspaces.get("missing")).rejects.toThrow(ApiError);

    try {
      await api.workspaces.get("missing");
    } catch (err) {
      expect(err).toBeInstanceOf(ApiError);
      expect((err as ApiError).status).toBe(404);
      expect((err as ApiError).code).toBe("not_found");
    }
  });

  it("sends POST with JSON body for create", async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: () =>
        Promise.resolve({
          data: { id: "ws-2", name: "test", status: "active", created_at: "", updated_at: "" },
        }),
    });

    await api.workspaces.create({ name: "test" });
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/workspaces",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ name: "test" }),
      }),
    );
  });

  it("builds query string for paginated list", async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: () => Promise.resolve({ data: { data: [], cursor: null } }),
    });

    await api.workspaces.list({ cursor: "abc", limit: 10 });
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/workspaces?cursor=abc&limit=10",
      expect.anything(),
    );
  });
});

describe("api.sessions", () => {
  it("builds workspace-scoped list URL", async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: () => Promise.resolve({ data: { data: [], cursor: null } }),
    });

    await api.sessions.list("ws-1");
    expect(mockFetch).toHaveBeenCalledWith("/api/workspaces/ws-1/sessions", expect.anything());
  });

  it("sends prompt via POST", async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      json: () => Promise.resolve({ data: { message_id: "msg-1" } }),
    });

    const result = await api.sessions.sendPrompt("sess-1", "hello");
    expect(result.message_id).toBe("msg-1");
    expect(mockFetch).toHaveBeenCalledWith(
      "/api/sessions/sess-1/prompt",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ prompt: "hello" }),
      }),
    );
  });
});
