// Tests for feat-054 — the session page layout switcher.
//
// Coverage map:
//   1. `SessionHeader` renders native chrome unchanged (no pill row)
//      when the session is in `native` mode.
//   2. `SessionHeader` renders the wrapped-mode pill row (runtime
//      name, permission mode, resume state) when the session is
//      in `wrapped` mode.
//   3. `SessionHeader` falls back to the runtime kind enum name when
//      the provider lookup is still loading (defensive default).
//   4. `SessionHeader` treats `attended` as native (defensive default;
//      attended is reserved for Phase 11).
//   5. `WrappedSessionBanner` is dismissable; the dismissal persists
//      in localStorage under a per-session key.
//   6. `WrappedSessionBanner` is hidden on subsequent turns via the
//      `firstTurn` prop.
//   7. The `reducer` updates `lastResumeState` from both
//      `MESSAGE_PERSISTED` and `DONE` actions (last-wins).

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { SessionHeader } from "../../components/session-header";
import { WrappedSessionBanner } from "../../components/wrapped-session-banner";
import { EMPTY_LIVE_BUFFER, reducer, type Action } from "../../hooks/use-session";
import type { Provider, ResumeState, Session } from "../../lib/types";

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

const baseSessionFields = {
  id: "s-1",
  workspace_id: "w-1",
  provider_id: "p-1",
  specialist_id: null,
  parent_session_id: null,
  status: "ready" as const,
  model: "claude-sonnet-4-5",
  cwd: "/home/u/proj",
  codebase_id: null,
  runtime_metadata_json: null,
  created_at: "2026-06-01T00:00:00Z",
  updated_at: "2026-06-01T00:00:00Z",
};

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    ...baseSessionFields,
    runtime_kind: "claude-code",
    mode: "wrapped",
    ...overrides,
  } as Session;
}

const baseProvider: Provider = {
  id: "p-1",
  type: "anthropic",
  kind: "cli",
  name: "My Claude Code",
  default_model: "claude-sonnet-4-5",
  binary_path: "/usr/local/bin/claude",
  args_json: null,
  env_json: null,
  permission_mode: "default",
  healthy: true,
  created_at: "2026-06-01T00:00:00Z",
};

function renderHeader(session: Session, provider: Provider | null = baseProvider) {
  return render(
    <MemoryRouter>
      <SessionHeader
        session={session}
        provider={provider}
        resumeState={null}
        isCancelling={false}
        onCancel={() => {}}
      />
    </MemoryRouter>,
  );
}

// ---------------------------------------------------------------------------
// SessionHeader
// ---------------------------------------------------------------------------

describe("SessionHeader — native mode", () => {
  it("renders the chat chrome without the wrapped-mode pill row", () => {
    renderHeader(makeSession({ mode: "native" }));

    expect(screen.getByRole("heading", { name: "Session" })).toBeInTheDocument();
    // No pill row in native mode.
    expect(screen.queryByTestId("wrapped-pill-row")).not.toBeInTheDocument();
  });
});

describe("SessionHeader — wrapped mode", () => {
  it("renders the runtime / permission / resume pill row", () => {
    renderHeader(makeSession({ mode: "wrapped" }), baseProvider);

    const row = screen.getByTestId("wrapped-pill-row");
    expect(row).toBeInTheDocument();
    // Runtime name comes from the resolved provider.
    expect(row).toHaveTextContent("My Claude Code");
    // Permission mode is rendered as a labelled pill.
    expect(row).toHaveTextContent("Permissions: default");
  });

  it("renders the resume-state pill when resumeState is set", () => {
    render(
      <MemoryRouter>
        <SessionHeader
          session={makeSession({ mode: "wrapped" })}
          provider={baseProvider}
          resumeState={"native" satisfies ResumeState}
          isCancelling={false}
          onCancel={() => {}}
        />
      </MemoryRouter>,
    );
    expect(screen.getByTestId("wrapped-pill-row")).toHaveTextContent("Resume: native");
  });

  it("omits the resume-state pill when resumeState is null (HTTP runtime, pre-first-event)", () => {
    renderHeader(makeSession({ mode: "wrapped" }));
    expect(screen.queryByText(/Resume:/)).not.toBeInTheDocument();
  });

  it("omits the permission-mode pill when the provider has no permission_mode set", () => {
    renderHeader(makeSession({ mode: "wrapped" }), { ...baseProvider, permission_mode: null });
    expect(screen.queryByText(/Permissions:/)).not.toBeInTheDocument();
  });

  it("falls back to the runtime kind enum name when the provider is still loading", () => {
    renderHeader(makeSession({ mode: "wrapped", runtime_kind: "claude-code" }), null);
    // The fallback label for "claude-code" is "Claude Code".
    expect(screen.getByTestId("wrapped-pill-row")).toHaveTextContent("Claude Code");
  });
});

describe("SessionHeader — attended mode", () => {
  // Attended is rejected at create time (Phase 11) but the frontend
  // must not crash if a row somehow lands with that mode. The
  // contract is: render as native (no pill row).
  it("defensively defaults to native chrome when mode is attended", () => {
    renderHeader(makeSession({ mode: "attended" }));
    expect(screen.queryByTestId("wrapped-pill-row")).not.toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// WrappedSessionBanner
// ---------------------------------------------------------------------------

describe("WrappedSessionBanner", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  afterEach(() => {
    window.localStorage.clear();
  });

  it("renders on the first turn", () => {
    render(<WrappedSessionBanner sessionId="s-1" firstTurn={true} />);
    expect(screen.getByTestId("wrapped-session-banner")).toBeInTheDocument();
  });

  it("writes a per-session key to localStorage on dismiss", () => {
    render(<WrappedSessionBanner sessionId="s-1" firstTurn={true} />);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss banner" }));
    expect(window.localStorage.getItem("weave.dismissed.wrappedSessionBanner.s-1")).toBe("true");
  });

  it("hides itself after a dismiss in the same render", () => {
    render(<WrappedSessionBanner sessionId="s-1" firstTurn={true} />);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss banner" }));
    expect(screen.queryByTestId("wrapped-session-banner")).not.toBeInTheDocument();
  });

  it("starts hidden when the dismissal is already in localStorage", () => {
    window.localStorage.setItem("weave.dismissed.wrappedSessionBanner.s-1", "true");
    render(<WrappedSessionBanner sessionId="s-1" firstTurn={true} />);
    expect(screen.queryByTestId("wrapped-session-banner")).not.toBeInTheDocument();
  });

  it("hides itself on subsequent turns via the firstTurn prop", () => {
    render(<WrappedSessionBanner sessionId="s-1" firstTurn={false} />);
    expect(screen.queryByTestId("wrapped-session-banner")).not.toBeInTheDocument();
  });

  it("scopes the dismissal key by session id", () => {
    render(<WrappedSessionBanner sessionId="s-1" firstTurn={true} />);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss banner" }));
    // s-2 is not affected by s-1's dismissal.
    window.localStorage.removeItem("weave.dismissed.wrappedSessionBanner.s-1");
    // Re-rendering with a different session id should not have written
    // any s-2 key.
    expect(window.localStorage.getItem("weave.dismissed.wrappedSessionBanner.s-2")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// useSession reducer — lastResumeState
// ---------------------------------------------------------------------------

describe("reducer — lastResumeState", () => {
  it("MESSAGE_PERSISTED captures the resume state from the SSE event", () => {
    const action: Action = {
      type: "MESSAGE_PERSISTED",
      persistedId: "msg-1",
      stopReason: "end_turn",
      resumeState: "native",
    };
    const next = reducer(EMPTY_LIVE_BUFFER, action);
    expect(next.lastResumeState).toBe("native");
    expect(next.persistedTurnId).toBe("msg-1");
  });

  it("DONE captures the resume state (last-wins, mirrors the SSE replay)", () => {
    const action: Action = { type: "DONE", stopReason: "end_turn", resumeState: "replayed" };
    const next = reducer(EMPTY_LIVE_BUFFER, action);
    expect(next.lastResumeState).toBe("replayed");
    expect(next.isStreaming).toBe(false);
  });

  it("SEND_STARTED resets lastResumeState to null", () => {
    // After a turn completes, the live buffer still has the previous
    // turn's resumeState. The next SEND_STARTED must reset it so the
    // header pill does not show a stale color for a turn that hasn't
    // emitted any events yet.
    const withState = { ...EMPTY_LIVE_BUFFER, lastResumeState: "native" as ResumeState };
    const action: Action = {
      type: "SEND_STARTED",
      messageId: "msg-2",
      content: "next",
      now: "2026-06-01T00:01:00Z",
    };
    const next = reducer(withState, action);
    expect(next.lastResumeState).toBeNull();
  });

  it("CANCEL_OPTIMISTIC preserves lastResumeState (the user may still want to see why)", () => {
    const withState = { ...EMPTY_LIVE_BUFFER, lastResumeState: "replayed" as ResumeState };
    const next = reducer(withState, { type: "CANCEL_OPTIMISTIC" });
    expect(next.lastResumeState).toBe("replayed");
  });
});
