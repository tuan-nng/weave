import type { QueryClient } from "@tanstack/react-query";
import { describe, expect, test, vi } from "vitest";
import { queryKeys } from "../../lib/query-keys";
import {
  EMPTY_LIVE_BUFFER,
  invalidateCommittedTraceQueries,
  makeSseListener,
  reducer,
  type Action,
} from "../use-session";

/**
 * Reducer unit tests. The reducer is the single source of truth for
 * live streaming state in `useSession`. These tests pin the contract
 * that the page depends on: in particular, the id-based handoff
 * between live and persisted bubbles, which is what eliminates the
 * flash-on-completion and duplicate-bubble bugs.
 */
describe("invalidateCommittedTraceQueries", () => {
  test("refreshes all Journey sidebar trace groups", () => {
    const invalidateQueries = vi.fn();
    const qc = { invalidateQueries } as unknown as QueryClient;

    invalidateCommittedTraceQueries(qc, "s1");

    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: queryKeys.traces.journey("s1"),
    });
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: queryKeys.traces.fileChanges("s1"),
    });
    expect(invalidateQueries).toHaveBeenCalledWith({
      queryKey: queryKeys.traces.toolCalls("s1"),
    });
  });
});

describe("useSession reducer", () => {
  describe("SEND_STARTED", () => {
    test("resets to a clean streaming state", () => {
      const prev = {
        ...EMPTY_LIVE_BUFFER,
        streamId: "stream-99",
        textChunks: ["leftover", "text"],
        isStreaming: true,
        stopReason: "end_turn",
      };
      const next = reducer(prev, {
        type: "SEND_STARTED",
        messageId: "m1",
        content: "hi",
        now: "2026-06-01T00:00:00Z",
      } as Action);
      expect(next.streamId).toBeNull();
      expect(next.persistedTurnId).toBeNull();
      expect(next.textChunks).toEqual([]);
      expect(next.toolCalls.size).toBe(0);
      expect(next.thinking).toEqual([]);
      expect(next.isStreaming).toBe(true);
      expect(next.stopReason).toBeNull();
    });

    test("is the only place that resets the buffer", () => {
      // The page must rely on SEND_STARTED to clear stale state from
      // a previous turn. If a future refactor accidentally also
      // resets on a later event, duplicates can appear.
      const next = reducer(EMPTY_LIVE_BUFFER, {
        type: "TEXT_DELTA",
        text: "first chunk of new turn",
      } as Action);
      // TEXT_DELTA does NOT reset; it just appends.
      expect(next.textChunks).toEqual(["first chunk of new turn"]);
    });
  });

  describe("TEXT_DELTA", () => {
    test("appends chunks and flips isStreaming", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "a" } as Action);
      s = reducer(s, { type: "TEXT_DELTA", text: "b" } as Action);
      s = reducer(s, { type: "TEXT_DELTA", text: "c" } as Action);
      expect(s.textChunks).toEqual(["a", "b", "c"]);
      expect(s.isStreaming).toBe(true);
    });

    test("generates a streamId on the first chunk of a turn", () => {
      const s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "x" } as Action);
      expect(s.streamId).not.toBeNull();
      expect(s.streamId).toMatch(/^stream-\d+$/);
    });

    test("preserves an existing streamId across subsequent deltas", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "x" } as Action);
      const firstId = s.streamId;
      s = reducer(s, { type: "TEXT_DELTA", text: "y" } as Action);
      expect(s.streamId).toBe(firstId);
    });
  });

  describe("TOOL_USE_START", () => {
    test("adds a running tool call to the map", () => {
      const s = reducer(EMPTY_LIVE_BUFFER, {
        type: "TOOL_USE_START",
        id: "tool-1",
        name: "fs_read",
        input: { path: "/x" },
      } as Action);
      expect(s.toolCalls.size).toBe(1);
      const tc = s.toolCalls.get("tool-1");
      expect(tc?.status).toBe("running");
      expect(tc?.result).toBeNull();
    });

    test("multiple tool calls are tracked independently", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, {
        type: "TOOL_USE_START",
        id: "t1",
        name: "fs_read",
        input: {},
      } as Action);
      s = reducer(s, { type: "TOOL_USE_START", id: "t2", name: "fs_write", input: {} } as Action);
      expect(s.toolCalls.size).toBe(2);
      expect(s.toolCalls.get("t1")?.name).toBe("fs_read");
      expect(s.toolCalls.get("t2")?.name).toBe("fs_write");
    });
  });

  describe("TOOL_USE_DELTA", () => {
    test("appends delta to existing tool call's string input", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, {
        type: "TOOL_USE_START",
        id: "t1",
        name: "shell",
        input: { cmd: "" },
      } as Action);
      s = reducer(s, { type: "TOOL_USE_DELTA", id: "t1", delta: "ls" } as Action);
      s = reducer(s, { type: "TOOL_USE_DELTA", id: "t1", delta: " -la" } as Action);
      expect(s.toolCalls.get("t1")?.input).toBe("ls -la");
    });

    test("is a no-op for unknown tool ids", () => {
      const s = reducer(EMPTY_LIVE_BUFFER, {
        type: "TOOL_USE_DELTA",
        id: "missing",
        delta: "x",
      } as Action);
      expect(s.toolCalls.size).toBe(0);
    });
  });

  describe("TOOL_RESULT", () => {
    test("marks a running tool call as complete with its result", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, {
        type: "TOOL_USE_START",
        id: "t1",
        name: "shell",
        input: {},
      } as Action);
      s = reducer(s, { type: "TOOL_RESULT", id: "t1", result: "ok" } as Action);
      const tc = s.toolCalls.get("t1");
      expect(tc?.status).toBe("complete");
      expect(tc?.result).toBe("ok");
    });
  });

  describe("THINKING", () => {
    test("appends a new thinking block on the first event of a turn", () => {
      const s = reducer(EMPTY_LIVE_BUFFER, { type: "THINKING", text: "reasoning..." } as Action);
      expect(s.thinking).toHaveLength(1);
      expect(s.thinking[0].text).toBe("reasoning...");
      expect(s.thinking[0].expanded).toBe(false);
    });

    test("appends to the most recent block on subsequent events", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "THINKING", text: "first " } as Action);
      s = reducer(s, { type: "THINKING", text: "second" } as Action);
      expect(s.thinking).toHaveLength(1);
      expect(s.thinking[0].text).toBe("first second");
    });
  });

  describe("MESSAGE_PERSISTED (id-based handoff)", () => {
    test("sets persistedTurnId and stopReason, but keeps the live state intact", () => {
      // The point of the re-implementation: persistedTurnId is the
      // signal that closes the live bubble. The textChunks and
      // toolCalls stay in state so a re-render that arrives before
      // the history query refetch lands doesn't blank the screen.
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "hello" } as Action);
      const liveStreamId = s.streamId;
      s = reducer(s, {
        type: "MESSAGE_PERSISTED",
        persistedId: "msg-uuid-1",
        stopReason: "end_turn",
      } as Action);
      expect(s.streamId).toBe(liveStreamId);
      expect(s.persistedTurnId).toBe("msg-uuid-1");
      expect(s.textChunks).toEqual(["hello"]);
      expect(s.stopReason).toBe("end_turn");
    });

    test("is the signal the page uses to hide the live bubble", () => {
      // Page contract: LiveAssistantMessage returns null when
      // streamId === persistedTurnId. Verify the reducer
      // establishes that equality.
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "x" } as Action);
      // streamId is set; persistedTurnId is null
      expect(s.persistedTurnId).toBeNull();
      s = reducer(s, {
        type: "MESSAGE_PERSISTED",
        persistedId: s.streamId!, // server returned the same id? (sentinel)
        stopReason: "end_turn",
      } as Action);
      // persistedTurnId is now set; the page's id equality test
      // would be: streamId !== persistedTurnId. The server
      // returns the DATABASE id, not the streamId, so in practice
      // they're different — and the live bubble stays until
      // messages[] contains the row keyed by persistedTurnId.
      expect(s.persistedTurnId).toBe(s.streamId);
    });

    test("idempotent on duplicate MESSAGE_PERSISTED (reconnect replay)", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "x" } as Action);
      s = reducer(s, {
        type: "MESSAGE_PERSISTED",
        persistedId: "p1",
        stopReason: "end_turn",
      } as Action);
      const after = s;
      s = reducer(s, {
        type: "MESSAGE_PERSISTED",
        persistedId: "p1",
        stopReason: "end_turn",
      } as Action);
      expect(s).toEqual(after);
    });
  });

  describe("DONE", () => {
    test("flips isStreaming false and captures stopReason", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "x" } as Action);
      s = reducer(s, { type: "DONE", stopReason: "end_turn" } as Action);
      expect(s.isStreaming).toBe(false);
      expect(s.stopReason).toBe("end_turn");
    });

    test("works with null stopReason", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "x" } as Action);
      s = reducer(s, { type: "DONE", stopReason: null } as Action);
      expect(s.isStreaming).toBe(false);
      expect(s.stopReason).toBeNull();
    });
  });

  describe("ERROR", () => {
    test("flips isStreaming false and records the error reason", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "partial" } as Action);
      s = reducer(s, { type: "ERROR", stopReason: "error" } as Action);
      expect(s.isStreaming).toBe(false);
      expect(s.stopReason).toBe("error");
      // The partial text is intentionally preserved — the server has
      // already persisted it, and the live bubble stays visible
      // until MESSAGE_PERSISTED arrives.
      expect(s.textChunks).toEqual(["partial"]);
    });
  });

  describe("CANCEL_OPTIMISTIC", () => {
    test("flips isStreaming false, marks stopReason cancelled, preserves text", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "halfway" } as Action);
      s = reducer(s, { type: "CANCEL_OPTIMISTIC" } as Action);
      expect(s.isStreaming).toBe(false);
      expect(s.stopReason).toBe("cancelled");
      expect(s.textChunks).toEqual(["halfway"]);
    });
  });

  describe("SEND_FAILED", () => {
    test("resets to idle state with send_error stopReason", () => {
      const prev = {
        ...EMPTY_LIVE_BUFFER,
        textChunks: ["stale"],
        isStreaming: true,
      };
      const s = reducer(prev, { type: "SEND_FAILED" } as Action);
      expect(s.textChunks).toEqual([]);
      expect(s.isStreaming).toBe(false);
      expect(s.stopReason).toBe("send_error");
      expect(s.streamId).toBeNull();
    });
  });

  describe("purity", () => {
    test("does not mutate the input state", () => {
      const prev = {
        ...EMPTY_LIVE_BUFFER,
        textChunks: ["a"],
        toolCalls: new Map([
          ["t1", { id: "t1", name: "x", input: {}, result: null, status: "running" as const }],
        ]),
      };
      const prevTextChunksRef = prev.textChunks;
      const prevToolCallsRef = prev.toolCalls;
      reducer(prev, { type: "TEXT_DELTA", text: "b" } as Action);
      // The previous state object should be untouched.
      expect(prev.textChunks).toBe(prevTextChunksRef);
      expect(prev.textChunks).toEqual(["a"]);
      expect(prev.toolCalls).toBe(prevToolCallsRef);
      expect(prev.toolCalls.size).toBe(1);
    });

    test("returns a new state object", () => {
      const prev = EMPTY_LIVE_BUFFER;
      const next = reducer(prev, { type: "TEXT_DELTA", text: "x" } as Action);
      expect(next).not.toBe(prev);
    });
  });

  describe("end-to-end happy path", () => {
    test("a full turn: send → text deltas → message_persisted → done", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, {
        type: "SEND_STARTED",
        messageId: "user-1",
        content: "hello",
        now: "2026-06-01T00:00:00Z",
      } as Action);
      expect(s.isStreaming).toBe(true);
      expect(s.streamId).toBeNull();

      s = reducer(s, { type: "TEXT_DELTA", text: "Hel" } as Action);
      s = reducer(s, { type: "TEXT_DELTA", text: "lo!" } as Action);
      expect(s.textChunks).toEqual(["Hel", "lo!"]);

      s = reducer(s, {
        type: "MESSAGE_PERSISTED",
        persistedId: "asst-1",
        stopReason: "end_turn",
      } as Action);
      expect(s.persistedTurnId).toBe("asst-1");
      expect(s.isStreaming).toBe(true); // still true until DONE

      s = reducer(s, { type: "DONE", stopReason: "end_turn" } as Action);
      expect(s.isStreaming).toBe(false);
      expect(s.stopReason).toBe("end_turn");
    });
  });

  describe("end-to-end cancel path", () => {
    test("cancel mid-stream: text deltas → cancel → message_persisted → done", () => {
      let s = reducer(EMPTY_LIVE_BUFFER, { type: "TEXT_DELTA", text: "partial" } as Action);
      s = reducer(s, { type: "CANCEL_OPTIMISTIC" } as Action);
      s = reducer(s, {
        type: "MESSAGE_PERSISTED",
        persistedId: "asst-1",
        stopReason: "cancelled",
      } as Action);
      s = reducer(s, { type: "DONE", stopReason: "cancelled" } as Action);
      expect(s.persistedTurnId).toBe("asst-1");
      expect(s.isStreaming).toBe(false);
      expect(s.stopReason).toBe("cancelled");
      expect(s.textChunks).toEqual(["partial"]); // partial preserved
    });
  });
});

/**
 * makeSseListener regression tests.
 *
 * The `"error"` listener is the one that the previous bug bit:
 * EventSource's built-in `error` event fires for connection-level
 * problems with `e.data === undefined` AND for server-sent
 * `event: error` SSE messages with data. The listener must
 * distinguish the two: connection errors get dropped (the
 * `es.onerror` handler manages reconnect), server errors get
 * JSON-parsed and forwarded to the reducer.
 */
describe("makeSseListener", () => {
  test("'error' with no data (built-in connection error) does not call handleEvent", () => {
    const handleEvent = vi.fn();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const listener = makeSseListener("error", handleEvent);

    listener({ data: undefined } as MessageEvent);

    expect(handleEvent).not.toHaveBeenCalled();
    expect(warn).not.toHaveBeenCalled();
    warn.mockRestore();
  });

  test("'error' with server-sent JSON is forwarded to handleEvent", () => {
    const handleEvent = vi.fn();
    const listener = makeSseListener("error", handleEvent);

    listener({
      data: JSON.stringify({ type: "error", message: "session not found" }),
    } as MessageEvent);

    expect(handleEvent).toHaveBeenCalledWith("error", {
      type: "error",
      message: "session not found",
    });
  });

  test("'text_delta' with server-sent JSON is forwarded to handleEvent", () => {
    const handleEvent = vi.fn();
    const listener = makeSseListener("text_delta", handleEvent);

    listener({ data: JSON.stringify({ type: "text_delta", text: "hi" }) } as MessageEvent);

    expect(handleEvent).toHaveBeenCalledWith("text_delta", {
      type: "text_delta",
      text: "hi",
    });
  });

  test("'text_delta' with invalid JSON logs a warning and does not call handleEvent", () => {
    const handleEvent = vi.fn();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const listener = makeSseListener("text_delta", handleEvent);

    listener({ data: "not json{" } as MessageEvent);

    expect(handleEvent).not.toHaveBeenCalled();
    expect(warn).toHaveBeenCalledWith(
      "[useSession] Failed to parse SSE event:",
      "text_delta",
      "not json{",
      expect.any(Error),
    );
    warn.mockRestore();
  });
});
