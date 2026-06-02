import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useFileChanges } from "../use-file-changes";

vi.mock("../../lib/api", () => ({
  api: {
    traces: {
      fileChanges: vi.fn(),
    },
  },
}));

import { api } from "../../lib/api";
const mockApi = vi.mocked(api);

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  function Wrapper({ children }: { children: React.ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  }
  return Wrapper;
}

describe("useFileChanges", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("calls api.traces.fileChanges with the session id", async () => {
    const changes = [
      { path: "/tmp/a.rs", actions: ["write", "read"], count: 3 },
      { path: "/tmp/b.rs", actions: ["create"], count: 1 },
    ];
    mockApi.traces.fileChanges.mockResolvedValue(changes);

    const { result } = renderHook(() => useFileChanges("s1"), { wrapper: makeWrapper() });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(mockApi.traces.fileChanges).toHaveBeenCalledWith("s1");
    expect(result.current.data).toEqual(changes);
  });

  it("does not fetch when sessionId is empty", () => {
    renderHook(() => useFileChanges(""), { wrapper: makeWrapper() });
    expect(mockApi.traces.fileChanges).not.toHaveBeenCalled();
  });

  it("starts in loading state", () => {
    mockApi.traces.fileChanges.mockResolvedValue([]);
    const { result } = renderHook(() => useFileChanges("s1"), { wrapper: makeWrapper() });
    expect(result.current.isLoading).toBe(true);
    expect(result.current.data).toBeUndefined();
  });
});
