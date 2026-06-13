// Tests for the agent-status helpers (F-14).
//
// The hook itself is a thin TanStack Query wrapper, but the
// `describeSession` helper encodes the state machine that decides
// whether a card renders "Running" vs "Needs input" — that's the
// piece the user sees, so the logic deserves a direct test.

import { describe, expect, it } from "vitest";
import { describeSession } from "../use-agent-status";
import type { Session } from "../../lib/types";

function makeSession(overrides: Partial<Session> = {}): Session {
  return {
    id: "s-1",
    workspace_id: "w-1",
    provider_id: "p-1",
    specialist_id: null,
    parent_session_id: null,
    context_id: null,
    status: "ready",
    model: null,
    cwd: null,
    codebase_id: null,
    runtime_kind: "anthropic-api",
    mode: "native",
    runtime_metadata_json: null,
    last_message_role: null,
    awaiting_user_input: false,
    created_at: "2026-06-13T00:00:00Z",
    updated_at: "2026-06-13T00:00:00Z",
    ...overrides,
  };
}

describe("describeSession (F-14)", () => {
  it("returns unknown for a null session", () => {
    const result = describeSession(null);
    expect(result.state).toBe("unknown");
    expect(result.status).toBeNull();
  });

  it("returns running for a ready session with no messages", () => {
    const session = makeSession({ status: "ready", awaiting_user_input: false });
    const result = describeSession(session);
    expect(result.state).toBe("running");
    expect(result.label).toBe("Running");
    expect(result.tone).toBe("blue");
  });

  it("returns needs_input for a ready session awaiting user input", () => {
    const session = makeSession({
      status: "ready",
      awaiting_user_input: true,
      last_message_role: "assistant",
    });
    const result = describeSession(session);
    expect(result.state).toBe("needs_input");
    expect(result.label).toBe("Needs input");
    expect(result.tone).toBe("rose");
  });

  it("returns error for an errored session", () => {
    const session = makeSession({ status: "error" });
    const result = describeSession(session);
    expect(result.state).toBe("error");
    expect(result.label).toBe("Error");
    expect(result.tone).toBe("rose");
  });

  it("returns cancelled for a cancelled session", () => {
    const session = makeSession({ status: "cancelled" });
    const result = describeSession(session);
    expect(result.state).toBe("cancelled");
    expect(result.label).toBe("Cancelled");
    expect(result.tone).toBe("slate");
  });

  it("returns connecting for a connecting session", () => {
    const session = makeSession({ status: "connecting", awaiting_user_input: false });
    const result = describeSession(session);
    expect(result.state).toBe("connecting");
    expect(result.label).toBe("Connecting");
    expect(result.tone).toBe("blue");
  });
});
