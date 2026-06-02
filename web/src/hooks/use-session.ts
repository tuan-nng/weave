import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type {
  SseDoneEvent,
  SseEvent,
  SseMessagePersistedEvent,
  Session,
  Message,
  TraceRow,
} from "../lib/types";

// ---------------------------------------------------------------------------
// Live streaming types
// ---------------------------------------------------------------------------

export interface LiveToolCall {
  id: string;
  name: string;
  input: unknown;
  result: string | null;
  status: "running" | "complete";
}

export interface LiveThinkingBlock {
  /// Concatenated thinking text for this turn. The server streams one
  /// `thinking` event per chunk; we append to keep the rendered output
  /// cheap to derive.
  text: string;
  /// Whether the user has expanded this block in the UI. Default closed
  /// to keep the chat uncluttered while the agent reasons.
  expanded: boolean;
}

/// State for a single in-flight (or just-completed) turn. One per session.
///
/// The handoff from "live streaming bubble" to "persisted bubble" is
/// driven entirely by `streamId` and `persistedTurnId`:
///
/// - On the first streaming event of a new turn, the reducer generates
///   a fresh `streamId` (UUID) and stores it here.
/// - On the server's `message_persisted` event, the reducer stores the
///   row's database id in `persistedTurnId`.
/// - The page's `LiveAssistantMessage` renders when
///   `streamId !== null && streamId !== persistedTurnId` and returns
///   null otherwise. No content-string comparison is required — the
///   server told us by id.
export interface LiveBuffer {
  streamId: string | null;
  persistedTurnId: string | null;
  textChunks: string[];
  toolCalls: Map<string, LiveToolCall>;
  thinking: LiveThinkingBlock[];
  isStreaming: boolean;
  stopReason: string | null;
}

/**
 * @internal
 * Exported for unit testing only.
 */
export const EMPTY_LIVE_BUFFER: LiveBuffer = {
  streamId: null,
  persistedTurnId: null,
  textChunks: [],
  toolCalls: new Map(),
  thinking: [],
  isStreaming: false,
  stopReason: null,
};

/// A user prompt that was just sent but is not yet present in the
/// persisted history. Rendered optimistically above the live bubble so
/// the user sees their own message immediately. Each is dropped once
/// the history query returns a `Message` with the same `id`.
export interface PendingPrompt {
  id: string;
  content: string;
  createdAt: string;
}

// ---------------------------------------------------------------------------
// Reducer: a single discriminated action union drives the live state
// ---------------------------------------------------------------------------

export type Action =
  | { type: "SEND_STARTED"; messageId: string; content: string; now: string }
  | { type: "TEXT_DELTA"; text: string }
  | { type: "TOOL_USE_START"; id: string; name: string; input: unknown }
  | { type: "TOOL_USE_DELTA"; id: string; delta: string }
  | { type: "TOOL_RESULT"; id: string; result: string }
  | { type: "THINKING"; text: string }
  | { type: "MESSAGE_PERSISTED"; persistedId: string; stopReason: string | null }
  | { type: "DONE"; stopReason: string | null }
  | { type: "ERROR"; stopReason: string }
  | { type: "CANCEL_OPTIMISTIC" }
  | { type: "SEND_FAILED" };

/**
 * @internal
 * Exported for unit testing only. The hook is the public API; the
 * reducer is a pure function that is easy to test in isolation.
 */
export function reducer(state: LiveBuffer, action: Action): LiveBuffer {
  switch (action.type) {
    case "SEND_STARTED":
      // The user just submitted a new prompt. Reset the live buffer to
      // a fresh streaming state. The streamId is null until the first
      // server event arrives — this gives the page a clean "no live
      // bubble yet" frame while the user message is being persisted.
      // Importantly, this is the ONLY place that resets the buffer:
      // before this re-implementation, the reset was a side effect of
      // sendMutation.onSuccess that ran alongside the optimistic
      // pendingPrompt insert; the reducer makes that a single named
      // transition that is easy to test.
      return { ...EMPTY_LIVE_BUFFER, isStreaming: true, stopReason: null };

    case "TEXT_DELTA":
      // If a streamId hasn't been set yet (e.g. the very first event of
      // a turn is a text_delta), generate one. This keeps the live
      // bubble visible from the first token.
      return {
        ...state,
        streamId: state.streamId ?? freshStreamId(),
        textChunks: [...state.textChunks, action.text],
        isStreaming: true,
      };

    case "TOOL_USE_START": {
      const next = new Map(state.toolCalls);
      next.set(action.id, {
        id: action.id,
        name: action.name,
        input: action.input,
        result: null,
        status: "running",
      });
      return {
        ...state,
        streamId: state.streamId ?? freshStreamId(),
        toolCalls: next,
        isStreaming: true,
      };
    }

    case "TOOL_USE_DELTA": {
      const next = new Map(state.toolCalls);
      const existing = next.get(action.id);
      if (existing) {
        next.set(action.id, {
          ...existing,
          input: typeof existing.input === "string" ? existing.input + action.delta : action.delta,
        });
      }
      return { ...state, toolCalls: next };
    }

    case "TOOL_RESULT": {
      const next = new Map(state.toolCalls);
      const existing = next.get(action.id);
      if (existing) {
        next.set(action.id, {
          ...existing,
          result: action.result,
          status: "complete",
        });
      }
      return { ...state, toolCalls: next };
    }

    case "THINKING": {
      // Append to the current thinking block if it exists and is still
      // empty of the "completed" flag (we keep it simple: always
      // append into a single block per turn). Render-side decides
      // whether to show this; default is closed.
      const existing = state.thinking[state.thinking.length - 1];
      if (existing && !existing.expanded) {
        const trimmed = state.thinking.slice(0, -1);
        return {
          ...state,
          streamId: state.streamId ?? freshStreamId(),
          thinking: [...trimmed, { ...existing, text: existing.text + action.text }],
          isStreaming: true,
        };
      }
      return {
        ...state,
        streamId: state.streamId ?? freshStreamId(),
        thinking: [...state.thinking, { text: action.text, expanded: false }],
        isStreaming: true,
      };
    }

    case "MESSAGE_PERSISTED":
      // The server has written the assistant message to the database
      // and broadcast the row id. We mark the live bubble as
      // "superseded" — the page's LiveAssistantMessage reads
      // `persistedTurnId` and returns null when it matches the
      // current streamId. The textChunks and toolCalls stay in state
      // briefly so a re-render that arrives before the history query
      // refetch lands doesn't blank the screen.
      return {
        ...state,
        persistedTurnId: action.persistedId,
        stopReason: action.stopReason,
        // The stream is no longer actively delivering events; the
        // terminal Done event will arrive next and finalize isStreaming.
        isStreaming: state.isStreaming,
      };

    case "DONE":
      // Terminal event. Stop streaming, capture the server's stop_reason
      // for the history refetch to display on the persisted message.
      return {
        ...state,
        isStreaming: false,
        stopReason: action.stopReason,
      };

    case "ERROR":
      return {
        ...state,
        isStreaming: false,
        stopReason: action.stopReason,
      };

    case "CANCEL_OPTIMISTIC":
      // User clicked cancel; the server will broadcast MESSAGE_PERSISTED
      // + DONE shortly. Show the streaming state has stopped, but keep
      // the textChunks so the user can see what streamed so far until
      // the persisted (partial) message arrives.
      return {
        ...state,
        isStreaming: false,
        stopReason: "cancelled",
      };

    case "SEND_FAILED":
      // The send mutation rejected (e.g. session is terminal). Reset
      // to a clean idle state so the UI doesn't show a stale
      // "streaming" indicator.
      return { ...EMPTY_LIVE_BUFFER, isStreaming: false, stopReason: "send_error" };
  }
}

let streamIdCounter = 0;
function freshStreamId(): string {
  // A monotonic id is sufficient for the page's id-based handoff; we
  // don't need a UUID because the persistedTurnId (server-assigned
  // UUID) is what gates the swap. Using a counter keeps the reducer
  // pure and the equality check cheap.
  streamIdCounter += 1;
  return `stream-${streamIdCounter}`;
}

// ---------------------------------------------------------------------------
// Hook return type
// ---------------------------------------------------------------------------

export interface UseSessionResult {
  session: Session | undefined;
  messages: Message[];
  traces: TraceRow[];
  liveBuffer: LiveBuffer;
  pendingPrompts: PendingPrompt[];
  isLoading: boolean;
  isError: boolean;
  error: Error | null;
  sendPrompt: (prompt: string) => void;
  cancelSession: () => void;
  isSending: boolean;
  isCancelling: boolean;
  sendError: Error | null;
  cancelError: Error | null;
}

// ---------------------------------------------------------------------------
// useSession hook
// ---------------------------------------------------------------------------

export function useSession(sessionId: string): UseSessionResult {
  const qc = useQueryClient();
  // Capture qc in a ref so the SSE effect doesn't depend on it
  const qcRef = useRef(qc);
  qcRef.current = qc;

  // --- TanStack Query: initial data ---
  const sessionQuery = useQuery({
    queryKey: queryKeys.sessions.detail(sessionId),
    queryFn: () => api.sessions.get(sessionId),
  });

  const historyQuery = useQuery({
    queryKey: queryKeys.sessions.history(sessionId),
    queryFn: () => api.sessions.history(sessionId, { limit: 100 }),
  });

  const tracesQuery = useQuery({
    queryKey: queryKeys.traces.list(sessionId),
    queryFn: () => api.traces.list(sessionId),
  });

  // --- Live streaming state via reducer ---
  const [liveBuffer, dispatch] = useReducer(reducer, EMPTY_LIVE_BUFFER);

  // --- Pending user prompts (optimistic) ---
  // Prompts the user has just sent but which are not yet in the
  // persisted history. Added on `sendPrompt` success, removed once
  // the history query returns a matching message.
  const pendingRef = useRef<PendingPrompt[]>([]);
  const [pendingPrompts, setPendingPrompts] = useState<PendingPrompt[]>([]);

  // --- SSE connection ---
  useEffect(() => {
    const es = new EventSource(`/api/sessions/${sessionId}/stream`);

    const handleEvent = (type: string, data: unknown) => {
      const event = { type, ...(data as object) } as SseEvent;

      switch (event.type) {
        case "text_delta":
          dispatch({ type: "TEXT_DELTA", text: event.text });
          break;

        case "tool_use_start":
          dispatch({
            type: "TOOL_USE_START",
            id: event.id,
            name: event.name,
            input: event.input,
          });
          break;

        case "tool_use_delta":
          dispatch({ type: "TOOL_USE_DELTA", id: event.id, delta: event.delta });
          break;

        case "tool_result":
          dispatch({ type: "TOOL_RESULT", id: event.id, result: event.result });
          break;

        case "thinking":
          dispatch({ type: "THINKING", text: event.text });
          break;

        case "message_persisted": {
          // The server has written the assistant message. Mark the
          // live bubble as superseded. The history refetch (kicked
          // off by the terminal `done` event below) will produce the
          // persisted `Message` keyed by `event.id`, and the page's
          // `AssistantMessage` for that id will render instead of
          // `LiveAssistantMessage`. There is no race here: the
          // message is in the DB before this event was sent.
          const mp = event as SseMessagePersistedEvent;
          dispatch({
            type: "MESSAGE_PERSISTED",
            persistedId: mp.id,
            stopReason: mp.stop_reason,
          });
          // The journey sidebar (`useJourney` + `useFileChanges`)
          // depends on the trace events that the server has just
          // emitted. Invalidate here — at the exact moment the
          // assistant message is committed — so the sidebar's
          // timeline and file list refresh together with the chat.
          //
          // The backend awaits the trace flush task before
          // broadcasting `message_persisted` (see
          // `run_prompt_task` in `service/sessions.rs`), so by the
          // time the client sees this event every new trace row is
          // already in SQLite. The refetch is guaranteed to see
          // them; there is no race.
          qcRef.current.invalidateQueries({
            queryKey: queryKeys.traces.journey(sessionId),
          });
          qcRef.current.invalidateQueries({
            queryKey: queryKeys.traces.fileChanges(sessionId),
          });
          break;
        }

        case "done": {
          // Terminal event. The server's MessageStore::create has
          // already run (see SseWireEvent::MessagePersisted above),
          // so invalidating the history query will refetch a list
          // that contains the new row — no race, no flash.
          qcRef.current.invalidateQueries({
            queryKey: queryKeys.sessions.detail(sessionId),
          });
          qcRef.current.invalidateQueries({
            queryKey: queryKeys.sessions.history(sessionId),
          });
          qcRef.current.invalidateQueries({
            queryKey: queryKeys.traces.list(sessionId),
          });
          const dr = (event as SseDoneEvent).stop_reason ?? null;
          dispatch({ type: "DONE", stopReason: dr });
          break;
        }

        case "error":
          // Mid-stream error from the agent. The server has already
          // persisted the partial text (with stop_reason="error" in
          // the metadata) and broadcast MessagePersisted, so the
          // error event here is purely informational.
          dispatch({ type: "ERROR", stopReason: "error" });
          break;

        case "connected":
          // The SSE server sends `connected` on every (re)connect —
          // including the natural reconnect that happens when the
          // server closes the stream at the end of a turn. The
          // reducer is the single source of truth for streaming
          // state, so a `connected` event has no reducer effect.
          // The new design eliminates the bug class this comment
          // used to guard against (the live buffer was being wiped
          // here, which erased streamed text on the natural
          // end-of-turn reconnect).
          break;

        case "gap":
          // Protocol event — no UI action needed
          break;
      }
    };

    const eventTypes: SseEvent["type"][] = [
      "text_delta",
      "tool_use_start",
      "tool_use_delta",
      "tool_result",
      "thinking",
      "message_persisted",
      "done",
      "error",
      "connected",
      "gap",
    ];

    for (const type of eventTypes) {
      es.addEventListener(type, (e: MessageEvent) => {
        try {
          const data = JSON.parse(e.data);
          handleEvent(type, data);
        } catch (err) {
          console.warn("[useSession] Failed to parse SSE event:", type, e.data, err);
        }
      });
    }

    es.onerror = () => {
      // Browser auto-reconnects — no manual handling needed. The
      // server will replay buffered events from the last seen id
      // and the reducer is idempotent (TEXT_DELTA appends in
      // order; MESSAGE_PERSISTED sets persistedTurnId; DONE flips
      // isStreaming). On a fresh page load (no Last-Event-ID
      // header) the server skips the buffer entirely.
    };

    return () => {
      es.close();
    };
  }, [sessionId]);

  // --- Mutations ---
  const sendMutation = useMutation({
    mutationFn: (prompt: string) => api.sessions.sendPrompt(sessionId, prompt),
    onSuccess: (data, prompt) => {
      // Add the user's own message to the pending list so it
      // appears immediately in the chat, above the streaming
      // assistant bubble.
      const pending: PendingPrompt = {
        id: data.message_id,
        content: prompt,
        createdAt: new Date().toISOString(),
      };
      pendingRef.current = [...pendingRef.current, pending];
      setPendingPrompts(pendingRef.current);
      // Reset the live buffer to a clean streaming state. The
      // reducer's SEND_STARTED action is the only place this
      // happens; before this re-implementation, the reset was a
      // side effect here that ran alongside the optimistic
      // pendingPrompt insert, which is harder to reason about and
      // easier to forget in a future refactor.
      dispatch({
        type: "SEND_STARTED",
        messageId: data.message_id,
        content: prompt,
        now: new Date().toISOString(),
      });
    },
    onError: () => {
      dispatch({ type: "SEND_FAILED" });
    },
  });

  const cancelMutation = useMutation({
    mutationFn: () => api.sessions.cancel(sessionId),
    onSuccess: () => {
      // Optimistic UI: mark the live state as no longer streaming.
      // The server's MESSAGE_PERSISTED + DONE events will arrive
      // shortly and reconcile the actual persisted (partial)
      // message.
      dispatch({ type: "CANCEL_OPTIMISTIC" });
    },
  });

  // --- Derived state ---
  const messages = useMemo(() => historyQuery.data?.data ?? [], [historyQuery.data]);
  const isLoading = sessionQuery.isLoading || historyQuery.isLoading || tracesQuery.isLoading;
  const isError = sessionQuery.isError || historyQuery.isError || tracesQuery.isError;
  const error =
    (sessionQuery.error as Error) ||
    (historyQuery.error as Error) ||
    (tracesQuery.error as Error) ||
    null;

  // --- Dedup pending prompts ---
  // When the history query returns, drop any pending prompt whose id
  // is now present in the persisted messages. Runs on every history
  // refetch (initial load, after each `done`, on window focus).
  useEffect(() => {
    if (pendingRef.current.length === 0) return;
    const persisted = new Set(messages.map((m) => m.id));
    const stillPending = pendingRef.current.filter((p) => !persisted.has(p.id));
    if (stillPending.length !== pendingRef.current.length) {
      pendingRef.current = stillPending;
      setPendingPrompts(stillPending);
    }
  }, [messages, setPendingPrompts]);

  return {
    session: sessionQuery.data,
    messages,
    traces: tracesQuery.data ?? [],
    liveBuffer,
    pendingPrompts,
    isLoading,
    isError,
    error,
    sendPrompt: sendMutation.mutate,
    cancelSession: cancelMutation.mutate,
    isSending: sendMutation.isPending,
    isCancelling: cancelMutation.isPending,
    sendError: sendMutation.error as Error | null,
    cancelError: cancelMutation.error as Error | null,
  };
}
