import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useJourney } from "../use-journey";

vi.mock("../../lib/api", () => ({
  api: {
    traces: {
      journey: vi.fn(),
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

describe("useJourney", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("calls api.traces.journey with the session id", async () => {
    const journey = [
      {
        id: "t1",
        session_id: "s1",
        event_type: "decision",
        summary: "decided to use Rust",
        data_json: '{"text":"I will use Rust"}',
        timestamp: "2026-01-01T00:00:00Z",
      },
    ];
    mockApi.traces.journey.mockResolvedValue(journey);

    const { result } = renderHook(() => useJourney("s1"), { wrapper: makeWrapper() });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    expect(mockApi.traces.journey).toHaveBeenCalledWith("s1");
    expect(result.current.data).toEqual(journey);
  });

  it("does not fetch when sessionId is empty", () => {
    renderHook(() => useJourney(""), { wrapper: makeWrapper() });
    expect(mockApi.traces.journey).not.toHaveBeenCalled();
  });

  it("starts in loading state", () => {
    mockApi.traces.journey.mockResolvedValue([]);
    const { result } = renderHook(() => useJourney("s1"), { wrapper: makeWrapper() });
    expect(result.current.isLoading).toBe(true);
    expect(result.current.data).toBeUndefined();
  });
});
