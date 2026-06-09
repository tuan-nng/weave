import { describe, expect, it } from "vitest";
import { correlateTraces, parseTraceToolCallPayload } from "../pages/session";
import type { Message, TraceRow } from "../../lib/types";

const baseTrace: TraceRow = {
  id: "trace-1",
  session_id: "session-1",
  event_type: "tool_call",
  summary: "shell_exec",
  data_json: null,
  timestamp: "2026-01-01T00:01:00Z",
};

const messages: Message[] = [
  {
    id: "user-1",
    session_id: "session-1",
    role: "user",
    content: "run a command",
    metadata: null,
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "assistant-1",
    session_id: "session-1",
    role: "assistant",
    content: "done",
    metadata: null,
    created_at: "2026-01-01T00:02:00Z",
  },
];

describe("correlateTraces", () => {
  it("attaches turn traces emitted before persistence to the following assistant message", () => {
    const traceMap = correlateTraces(messages, [baseTrace]);

    expect(traceMap.get("user-1")).toBeUndefined();
    expect(traceMap.get("assistant-1")?.map((trace) => trace.id)).toEqual(["trace-1"]);
  });

  it("falls back to the preceding assistant for legacy traces emitted after persistence", () => {
    const traceMap = correlateTraces(messages, [
      {
        ...baseTrace,
        timestamp: "2026-01-01T00:03:00Z",
      },
    ]);

    expect(traceMap.get("assistant-1")?.map((trace) => trace.id)).toEqual(["trace-1"]);
  });

  it("does not attach legacy traces after an assistant to the next turn", () => {
    const multiTurnMessages: Message[] = [
      ...messages,
      {
        id: "user-2",
        session_id: "session-1",
        role: "user",
        content: "run another command",
        metadata: null,
        created_at: "2026-01-01T00:04:00Z",
      },
      {
        id: "assistant-2",
        session_id: "session-1",
        role: "assistant",
        content: "done again",
        metadata: null,
        created_at: "2026-01-01T00:05:00Z",
      },
    ];

    const traceMap = correlateTraces(multiTurnMessages, [
      {
        ...baseTrace,
        timestamp: "2026-01-01T00:03:00Z",
      },
    ]);

    expect(traceMap.get("assistant-1")?.map((trace) => trace.id)).toEqual(["trace-1"]);
    expect(traceMap.get("assistant-2")).toBeUndefined();
  });

  it("ignores non-tool traces in the inline message tool list", () => {
    const traceMap = correlateTraces(messages, [
      {
        ...baseTrace,
        id: "decision-1",
        event_type: "decision",
        summary: "reasoned about options",
      },
    ]);

    expect(traceMap.size).toBe(0);
  });
});

describe("parseTraceToolCallPayload", () => {
  it("parses the current trace payload shape", () => {
    const payload = parseTraceToolCallPayload({
      ...baseTrace,
      data_json: JSON.stringify({
        tool_name: "shell_exec",
        input: { command: "echo hello" },
        output: { stdout: "hello\n", exit_code: 0 },
      }),
    });

    expect(payload.toolName).toBe("shell_exec");
    expect(payload.input).toEqual({ command: "echo hello" });
    expect(payload.output).toContain('"stdout": "hello\\n"');
  });

  it("keeps backwards compatibility with legacy input_json/output_json payloads", () => {
    const payload = parseTraceToolCallPayload({
      ...baseTrace,
      data_json: JSON.stringify({
        tool_name: "list_tasks",
        input_json: JSON.stringify({ done: false }),
        output_json: JSON.stringify({ count: 2 }),
      }),
    });

    expect(payload.toolName).toBe("list_tasks");
    expect(payload.input).toEqual({ done: false });
    expect(payload.output).toContain('"count": 2');
  });

  it("falls back to the trace summary when data_json is corrupt", () => {
    const payload = parseTraceToolCallPayload({
      ...baseTrace,
      summary: "fallback_tool",
      data_json: "{not valid json",
    });

    expect(payload.toolName).toBe("fallback_tool");
    expect(payload.input).toEqual({});
    expect(payload.output).toBeNull();
  });
});
