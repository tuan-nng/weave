import { useCallback, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { SseDoneEvent, SseEvent, Session, Message, TraceRow } from "../lib/types";

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

export interface LiveBuffer {
  textChunks: string[];
  thinkingChunks: string[];
  toolCalls: Map<string, LiveToolCall>;
  isStreaming: boolean;
  stopReason: string | null;
}

const EMPTY_LIVE_BUFFER: LiveBuffer = {
  textChunks: [],
  thinkingChunks: [],
  toolCalls: new Map(),
  isStreaming: false,
  stopReason: null,
};

// ---------------------------------------------------------------------------
// Hook return type
// ---------------------------------------------------------------------------

export interface UseSessionResult {
  session: Session | undefined;
  messages: Message[];
  traces: TraceRow[];
  liveBuffer: LiveBuffer;
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

  // --- Live streaming state ---
  const [liveBuffer, setLiveBuffer] = useState<LiveBuffer>(EMPTY_LIVE_BUFFER);
  const bufferRef = useRef<LiveBuffer>(EMPTY_LIVE_BUFFER);

  const updateLiveBuffer = useCallback((updater: (prev: LiveBuffer) => LiveBuffer) => {
    bufferRef.current = updater(bufferRef.current);
    setLiveBuffer({ ...bufferRef.current });
  }, []);

  // --- SSE connection ---
  useEffect(() => {
    const es = new EventSource(`/api/sessions/${sessionId}/stream`);

    const handleEvent = (type: string, data: unknown) => {
      const event = { type, ...(data as object) } as SseEvent;

      switch (event.type) {
        case "text_delta":
          updateLiveBuffer((prev) => ({
            ...prev,
            textChunks: [...prev.textChunks, event.text],
            isStreaming: true,
          }));
          break;

        case "tool_use_start":
          updateLiveBuffer((prev) => {
            const next = new Map(prev.toolCalls);
            next.set(event.id, {
              id: event.id,
              name: event.name,
              input: event.input,
              result: null,
              status: "running",
            });
            return { ...prev, toolCalls: next, isStreaming: true };
          });
          break;

        case "tool_use_delta":
          // For streaming JSON input — append delta to existing tool call input
          updateLiveBuffer((prev) => {
            const next = new Map(prev.toolCalls);
            const existing = next.get(event.id);
            if (existing) {
              next.set(event.id, {
                ...existing,
                input:
                  typeof existing.input === "string" ? existing.input + event.delta : event.delta,
              });
            }
            return { ...prev, toolCalls: next };
          });
          break;

        case "tool_result":
          updateLiveBuffer((prev) => {
            const next = new Map(prev.toolCalls);
            const existing = next.get(event.id);
            if (existing) {
              next.set(event.id, {
                ...existing,
                result: event.result,
                status: "complete",
              });
            }
            return { ...prev, toolCalls: next };
          });
          break;

        case "thinking":
          updateLiveBuffer((prev) => ({
            ...prev,
            thinkingChunks: [...prev.thinkingChunks, event.text],
            isStreaming: true,
          }));
          break;

        case "done": {
          // Invalidate queries to refetch persisted data
          qcRef.current.invalidateQueries({ queryKey: queryKeys.sessions.detail(sessionId) });
          qcRef.current.invalidateQueries({ queryKey: queryKeys.sessions.history(sessionId) });
          qcRef.current.invalidateQueries({ queryKey: queryKeys.traces.list(sessionId) });
          // Preserve stop_reason from the done event
          const doneBuffer: LiveBuffer = {
            ...EMPTY_LIVE_BUFFER,
            stopReason: (event as SseDoneEvent).stop_reason ?? null,
          };
          bufferRef.current = doneBuffer;
          setLiveBuffer(doneBuffer);
          break;
        }

        case "error":
          updateLiveBuffer((prev) => ({
            ...prev,
            isStreaming: false,
            stopReason: "error",
          }));
          break;

        case "connected":
          // Reconnection happened — clear stale streaming state
          bufferRef.current = EMPTY_LIVE_BUFFER;
          setLiveBuffer(EMPTY_LIVE_BUFFER);
          break;

        case "gap":
          // Protocol event — no UI action needed
          break;
      }
    };

    // Use addEventListener for each SSE event type
    const eventTypes = [
      "text_delta",
      "tool_use_start",
      "tool_use_delta",
      "tool_result",
      "thinking",
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
      // Browser auto-reconnects — no manual handling needed
      updateLiveBuffer((prev) => ({ ...prev, isStreaming: false }));
    };

    return () => {
      es.close();
    };
  }, [sessionId, updateLiveBuffer]);

  // --- Mutations ---
  const sendMutation = useMutation({
    mutationFn: (prompt: string) => api.sessions.sendPrompt(sessionId, prompt),
    onSuccess: () => {
      updateLiveBuffer((prev) => ({ ...prev, isStreaming: true, stopReason: null }));
    },
    onError: () => {
      updateLiveBuffer((prev) => ({ ...prev, isStreaming: false, stopReason: "send_error" }));
    },
  });

  const cancelMutation = useMutation({
    mutationFn: () => api.sessions.cancel(sessionId),
    onSuccess: () => {
      updateLiveBuffer((prev) => ({ ...prev, isStreaming: false, stopReason: "cancelled" }));
    },
  });

  // --- Derived state ---
  const isLoading = sessionQuery.isLoading || historyQuery.isLoading || tracesQuery.isLoading;
  const isError = sessionQuery.isError || historyQuery.isError || tracesQuery.isError;
  const error =
    (sessionQuery.error as Error) ||
    (historyQuery.error as Error) ||
    (tracesQuery.error as Error) ||
    null;

  return {
    session: sessionQuery.data,
    messages: historyQuery.data?.data ?? [],
    traces: tracesQuery.data ?? [],
    liveBuffer,
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
