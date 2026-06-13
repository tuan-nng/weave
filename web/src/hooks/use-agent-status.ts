// useAgentStatus — light wrapper around `api.sessions.get` that the
// kanban card uses to render an Agent pill variant based on the bound
// session's status.
//
// F-14: the card's previous pill was a static "Agent" badge. The
// user reading the board has no way to tell whether the agent is
// "running" (status="ready" + a turn in flight), "needs input"
// (status="ready" + zero in-flight events + at least one persisted
// assistant message), "errored", or "cancelled". The hook returns a
// derived state with a clear semantic label + a color hint the
// card uses to switch the pill variant.
//
// The backend (`SessionStore`) computes `awaiting_user_input` and
// `last_message_role` in the same query as the session row, so the
// pill has all the data it needs from a single `GET /api/sessions/:id`
// — no extra history fetch per card.
//
// Cache strategy: one query per session id, deduped by TanStack
// Query. Boards with N cards bound to the same session get a single
// fetch. The query is `staleTime: 5s` so re-renders on SSE
// `task_updated` patches don't re-hit the API.

import { useQuery } from "@tanstack/react-query";
import { api } from "../lib/api";
import { queryKeys } from "../lib/query-keys";
import type { Session, SessionStatus } from "../lib/types";

export type AgentUiState =
  | "idle" // session not yet bound — no pill
  | "running" // status="ready" + no completed turn yet (live or just-spawned)
  | "needs_input" // status="ready" + the agent finished a turn; user must send the next prompt
  | "error" // status="error"
  | "cancelled" // status="cancelled"
  | "completed" // status="completed"
  | "connecting" // status="connecting"
  | "unknown"; // status came back as something we don't model

export interface AgentStatusInfo {
  state: AgentUiState;
  status: SessionStatus | null;
  /// Short label the pill renders (e.g. "Running", "Needs input").
  label: string;
  /// Color hint the card uses to switch the pill variant
  /// (matches the brand-* palette used elsewhere).
  tone: "neutral" | "amber" | "rose" | "emerald" | "blue" | "slate";
  /// Tooltip / aria-label for accessibility + hover.
  hint: string;
}

const STATUS_TONE: Record<SessionStatus, AgentStatusInfo["tone"]> = {
  ready: "amber",
  connecting: "blue",
  completed: "emerald",
  cancelled: "slate",
  error: "rose",
};

const STATUS_LABEL: Record<SessionStatus, string> = {
  ready: "Ready",
  connecting: "Connecting",
  completed: "Completed",
  cancelled: "Cancelled",
  error: "Error",
};

const STATUS_HINT: Record<SessionStatus, string> = {
  ready: "Session is ready",
  connecting: "Session is connecting",
  completed: "Session completed",
  cancelled: "Session was cancelled",
  error: "Session errored",
};

export function describeSession(session: Session | null | undefined): AgentStatusInfo {
  if (!session) {
    return { state: "unknown", status: null, label: "Agent", tone: "amber", hint: "Open session" };
  }
  const status = session.status as SessionStatus;
  if (status === "ready") {
    if (session.awaiting_user_input) {
      return {
        state: "needs_input",
        status,
        label: "Needs input",
        tone: "rose",
        hint: "Agent finished a turn and is waiting for your reply",
      };
    }
    return {
      state: "running",
      status,
      label: "Running",
      tone: "blue",
      hint: "Agent is working",
    };
  }
  return {
    state: status,
    status,
    label: STATUS_LABEL[status] ?? "Agent",
    tone: STATUS_TONE[status] ?? "amber",
    hint: STATUS_HINT[status] ?? `Open session ${session.id}`,
  };
}

export function useAgentStatus(
  sessionId: string | null | undefined,
  options: { enabled?: boolean } = {},
) {
  return useQuery({
    queryKey: sessionId ? queryKeys.sessions.detail(sessionId) : ["sessions", "noop"],
    queryFn: () => (sessionId ? api.sessions.get(sessionId) : Promise.resolve(null)),
    enabled: options.enabled !== false && Boolean(sessionId),
    staleTime: 5_000,
    // The session SSE stream (driven by the session page) is the
    // authoritative source. We just need a snapshot.
    refetchOnWindowFocus: false,
  });
}
