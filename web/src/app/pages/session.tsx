import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useSession } from "../../hooks/use-session";
import { ROUTES } from "../../lib/routes";
import { Spinner } from "../../components/spinner";
import { StatusBadge } from "../../components/status-badge";
import type { LiveBuffer } from "../../hooks/use-session";
import type { Message, TraceRow } from "../../lib/types";

// ---------------------------------------------------------------------------
// Trace-to-message correlation
// ---------------------------------------------------------------------------

function correlateTraces(messages: Message[], traces: TraceRow[]): Map<string, TraceRow[]> {
  const sorted = [...traces]
    .filter((t) => t.event_type === "tool_call")
    .sort((a, b) => a.timestamp.localeCompare(b.timestamp));

  const result = new Map<string, TraceRow[]>();
  if (messages.length === 0 || sorted.length === 0) return result;

  // Assign each trace to the message whose created_at is the closest preceding timestamp
  for (const trace of sorted) {
    let bestId: string | null = null;
    for (const msg of messages) {
      if (msg.created_at <= trace.timestamp) {
        bestId = msg.id;
      }
    }
    if (bestId) {
      const arr = result.get(bestId) ?? [];
      arr.push(trace);
      result.set(bestId, arr);
    }
  }
  return result;
}

// ---------------------------------------------------------------------------
// ToolCallBlock
// ---------------------------------------------------------------------------

function ToolCallBlock({
  toolName,
  input,
  output,
  status,
}: {
  toolName: string;
  input: unknown;
  output: string | null;
  status: "running" | "complete";
}) {
  const [expanded, setExpanded] = useState(status === "running");

  return (
    <div
      className={`mt-3 rounded-xl overflow-hidden border ${
        status === "running"
          ? "border-brand-blue-200/60 bg-brand-blue-50/40"
          : "border-slate-200 bg-slate-50"
      }`}
    >
      <button
        type="button"
        aria-expanded={expanded}
        aria-controls={`tool-output-${toolName}`}
        className="w-full flex items-center gap-3 px-4 py-3 text-left hover:bg-slate-100/60 transition-colors duration-150"
        onClick={() => setExpanded(!expanded)}
      >
        {/* Tool icon */}
        <div className="relative w-6 h-6 rounded-md bg-brand-blue-100 flex items-center justify-center flex-shrink-0">
          <svg
            className="w-3.5 h-3.5 text-brand-blue-600"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            {status === "running" ? (
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z"
              />
            ) : (
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"
              />
            )}
          </svg>
          {status === "running" && (
            <svg
              className="absolute inset-0 w-6 h-6 animate-spin"
              style={{ animationDuration: "2s" }}
              viewBox="0 0 24 24"
              fill="none"
            >
              <circle
                cx="12"
                cy="12"
                r="10"
                stroke="currentColor"
                strokeWidth="2"
                strokeDasharray="40 20"
                className="text-brand-blue-400"
                strokeLinecap="round"
              />
            </svg>
          )}
        </div>
        <span className="text-xs font-medium text-slate-700">{toolName}</span>
        <span className="flex-1" />
        {/* Status badge */}
        {status === "running" ? (
          <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-[10px] font-semibold bg-brand-blue-50 text-brand-blue-700 border border-brand-blue-200/60">
            <span className="w-1 h-1 rounded-full bg-brand-blue-400 animate-pulse" />
            Running
          </span>
        ) : (
          <span className="inline-flex items-center px-2 py-0.5 rounded-full text-[10px] font-semibold bg-brand-emerald-50 text-brand-emerald-700 border border-brand-emerald-200/60">
            Completed
          </span>
        )}
        {/* Chevron */}
        <svg
          className="w-4 h-4 text-slate-400 transition-transform duration-150"
          style={{ transform: expanded ? "rotate(90deg)" : "rotate(0deg)" }}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
        </svg>
      </button>
      {expanded && (
        <div className="px-4 pb-3 border-t border-slate-200/60">
          <p className="text-[10px] font-medium uppercase tracking-wider text-slate-400 mt-3 mb-2">
            Input
          </p>
          <pre className="font-mono text-[11px] text-slate-600 bg-white rounded-lg p-3 border border-slate-100 overflow-x-auto">
            {typeof input === "string" ? input : JSON.stringify(input, null, 2)}
          </pre>
          {output !== null && (
            <>
              <p className="text-[10px] font-medium uppercase tracking-wider text-slate-400 mt-3 mb-2">
                Output
              </p>
              <pre className="font-mono text-[11px] text-slate-500 bg-white rounded-lg p-3 border border-slate-100 overflow-x-auto max-h-48">
                {output}
              </pre>
            </>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// TraceToolCallBlock (from historical trace data)
// ---------------------------------------------------------------------------

function TraceToolCallBlock({ trace }: { trace: TraceRow }) {
  let data: Record<string, unknown> = {};
  try {
    data = trace.data_json ? JSON.parse(trace.data_json) : {};
  } catch {
    // corrupted trace data — fall back to summary
  }

  let parsedInput: unknown = {};
  try {
    parsedInput = data.input_json ? JSON.parse(data.input_json as string) : {};
  } catch {
    // corrupted input data
  }

  return (
    <ToolCallBlock
      toolName={(data.tool_name as string) ?? trace.summary}
      input={parsedInput}
      output={(data.output_json as string) ?? null}
      status="complete"
    />
  );
}

// ---------------------------------------------------------------------------
// StreamingIndicator
// ---------------------------------------------------------------------------

function StreamingIndicator() {
  return (
    <div className="flex items-center gap-1">
      <span
        className="w-1.5 h-1.5 rounded-full bg-slate-400 animate-bounce"
        style={{ animationDelay: "0ms" }}
      />
      <span
        className="w-1.5 h-1.5 rounded-full bg-slate-400 animate-bounce"
        style={{ animationDelay: "200ms" }}
      />
      <span
        className="w-1.5 h-1.5 rounded-full bg-slate-400 animate-bounce"
        style={{ animationDelay: "400ms" }}
      />
    </div>
  );
}

// ---------------------------------------------------------------------------
// UserMessage
// ---------------------------------------------------------------------------

function UserMessage({ message }: { message: Message }) {
  const time = new Date(message.created_at).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });

  return (
    <div className="flex justify-end animate-fade-in">
      <div className="max-w-[85%]">
        <div className="bg-brand-blue-500 text-white text-sm leading-relaxed px-4 py-3 rounded-2xl rounded-br-md shadow-sm whitespace-pre-wrap">
          {message.content}
        </div>
        <p className="text-[10px] text-slate-400 mt-1.5 text-right">{time}</p>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// AssistantMessage
// ---------------------------------------------------------------------------

function AssistantMessage({ message, traces }: { message: Message; traces: TraceRow[] }) {
  const time = new Date(message.created_at).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });

  return (
    <div className="flex justify-start animate-fade-in-up">
      <div className="w-full">
        {/* Avatar row */}
        <div className="flex items-center gap-2 mb-2">
          <div className="w-6 h-6 rounded-lg bg-gradient-to-br from-brand-orchid-400 to-brand-orchid-600 flex items-center justify-center">
            <svg
              className="w-3.5 h-3.5 text-white"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z"
              />
            </svg>
          </div>
          <span className="text-xs font-medium text-slate-500">Weave</span>
          <span className="text-[10px] text-slate-300">&middot;</span>
          <span className="text-[10px] text-slate-400">{time}</span>
        </div>
        {/* Message body */}
        <div className="bg-white border border-black/[0.06] rounded-2xl rounded-tl-md p-5 shadow-[0_1px_3px_rgba(0,0,0,0.04)]">
          <div className="prose prose-sm prose-slate max-w-none prose-pre:bg-slate-900 prose-pre:text-slate-100 prose-pre:rounded-xl prose-code:before:hidden prose-code:after:hidden">
            <Markdown remarkPlugins={[remarkGfm]}>{message.content}</Markdown>
          </div>
          {/* Tool calls from traces */}
          {traces.map((trace) => (
            <TraceToolCallBlock key={trace.id} trace={trace} />
          ))}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// LiveAssistantMessage (streaming in progress)
// ---------------------------------------------------------------------------

function LiveAssistantMessage({
  liveBuffer,
  isStreaming,
}: {
  liveBuffer: LiveBuffer;
  isStreaming: boolean;
}) {
  const text = liveBuffer.textChunks.join("");
  const toolCalls = Array.from(liveBuffer.toolCalls.values());

  if (!isStreaming && text.length === 0 && toolCalls.length === 0) return null;

  const time = new Date().toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });

  return (
    <div className="flex justify-start animate-fade-in-up">
      <div className="w-full">
        {/* Avatar row with streaming indicator */}
        <div className="flex items-center gap-2 mb-2">
          <div className="w-6 h-6 rounded-lg bg-gradient-to-br from-brand-orchid-400 to-brand-orchid-600 flex items-center justify-center">
            <svg
              className="w-3.5 h-3.5 text-white"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z"
              />
            </svg>
          </div>
          <span className="text-xs font-medium text-slate-500">Weave</span>
          <span className="text-[10px] text-slate-300">&middot;</span>
          <span className="text-[10px] text-slate-400">{time}</span>
          {isStreaming && (
            <>
              <StreamingIndicator />
              <span className="text-xs text-slate-400 ml-0.5">
                {toolCalls.some((t) => t.status === "running") ? "Working..." : "Thinking..."}
              </span>
            </>
          )}
        </div>
        {/* Message body */}
        <div className="bg-white border border-black/[0.06] rounded-2xl rounded-tl-md p-5 shadow-[0_1px_3px_rgba(0,0,0,0.04)]">
          {text.length > 0 && (
            <div className="prose prose-sm prose-slate max-w-none prose-pre:bg-slate-900 prose-pre:text-slate-100 prose-pre:rounded-xl prose-code:before:hidden prose-code:after:hidden">
              <Markdown remarkPlugins={[remarkGfm]}>{text}</Markdown>
            </div>
          )}
          {/* Live tool calls */}
          {toolCalls.map((tc) => (
            <ToolCallBlock
              key={tc.id}
              toolName={tc.name}
              input={tc.input}
              output={tc.result}
              status={tc.status}
            />
          ))}
          {isStreaming && text.length === 0 && toolCalls.length === 0 && (
            <div className="flex items-center gap-2 py-2">
              <StreamingIndicator />
              <span className="text-xs text-slate-400">Thinking...</span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// MessageInput
// ---------------------------------------------------------------------------

function MessageInput({
  onSend,
  disabled,
  isSending,
}: {
  onSend: (prompt: string) => void;
  disabled: boolean;
  isSending: boolean;
}) {
  const [value, setValue] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Auto-resize
  useEffect(() => {
    const el = textareaRef.current;
    if (el) {
      el.style.height = "auto";
      el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
    }
  }, [value]);

  const handleSubmit = useCallback(() => {
    const trimmed = value.trim();
    if (!trimmed || disabled || isSending) return;
    onSend(trimmed);
    setValue("");
  }, [value, disabled, isSending, onSend]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSubmit();
      }
    },
    [handleSubmit],
  );

  return (
    <div className="flex-shrink-0 border-t border-slate-200/80 bg-white/80 backdrop-blur-sm">
      <div className="max-w-3xl mx-auto px-5 py-4">
        <div className="relative">
          <label htmlFor="session-message-input" className="sr-only">
            Message
          </label>
          <textarea
            id="session-message-input"
            ref={textareaRef}
            rows={1}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={disabled ? "Session has ended" : "Send a message..."}
            disabled={disabled}
            className="w-full resize-none bg-white border border-slate-200 rounded-2xl text-sm text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150 px-4 py-3 pr-12 shadow-[0_1px_3px_rgba(0,0,0,0.04)] disabled:bg-slate-50 disabled:text-slate-400"
            style={{ minHeight: "48px", maxHeight: "200px" }}
          />
          <button
            type="button"
            aria-label="Send message"
            onClick={handleSubmit}
            disabled={disabled || isSending || value.trim().length === 0}
            className="absolute right-2 bottom-2 w-8 h-8 flex items-center justify-center rounded-xl bg-brand-blue-500 text-white hover:bg-brand-blue-600 focus:outline-none focus:ring-2 focus:ring-brand-blue-500 focus:ring-offset-2 transition-all duration-150 shadow-sm hover:shadow-md disabled:bg-slate-200 disabled:text-slate-400 disabled:shadow-none"
          >
            <svg
              className="w-4 h-4"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M12 19V5m0 0l-7 7m7-7l7 7" />
            </svg>
          </button>
        </div>
        <p className="text-[10px] text-slate-300 mt-2 text-center">
          Weave may make mistakes. Verify important outputs.
        </p>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// SessionPage
// ---------------------------------------------------------------------------

export default function SessionPage() {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const sessionId = id ?? "";

  const {
    session,
    messages,
    traces,
    liveBuffer,
    isLoading,
    isError,
    error,
    sendPrompt,
    cancelSession,
    isSending,
    isCancelling,
  } = useSession(sessionId);

  // Auto-scroll
  const scrollContainerRef = useRef<HTMLDivElement>(null);
  const scrollTargetRef = useRef<HTMLDivElement>(null);
  const isAtBottomRef = useRef(true);

  const scrollToBottom = useCallback(() => {
    scrollTargetRef.current?.scrollIntoView({ behavior: "smooth" });
  }, []);

  // Track scroll position
  useEffect(() => {
    const container = scrollContainerRef.current;
    if (!container) return;

    const handleScroll = () => {
      const { scrollTop, scrollHeight, clientHeight } = container;
      isAtBottomRef.current = scrollHeight - scrollTop - clientHeight < 100;
    };

    container.addEventListener("scroll", handleScroll, { passive: true });
    return () => container.removeEventListener("scroll", handleScroll);
  }, []);

  // Auto-scroll on new content
  const contentLength = messages.length + liveBuffer.textChunks.length;
  useEffect(() => {
    if (isAtBottomRef.current) {
      scrollToBottom();
    }
  }, [contentLength, scrollToBottom]);

  // Scroll to bottom when sending a prompt
  const handleSend = useCallback(
    (prompt: string) => {
      sendPrompt(prompt);
      // Force scroll to bottom on next tick
      setTimeout(scrollToBottom, 50);
    },
    [sendPrompt, scrollToBottom],
  );

  // Correlate traces to messages (memoized — O(n*m) computation)
  const traceMap = useMemo(() => correlateTraces(messages, traces), [messages, traces]);

  // Missing session ID guard (after hooks to satisfy rules-of-hooks)
  if (!id) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-sm text-slate-500">Missing session ID</p>
      </div>
    );
  }

  // Terminal state check
  const isTerminal =
    session?.status === "completed" ||
    session?.status === "cancelled" ||
    session?.status === "error" ||
    liveBuffer.stopReason !== null;

  // Loading
  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner />
      </div>
    );
  }

  // Error
  if (isError) {
    return (
      <div className="flex items-center justify-center h-full">
        <div className="text-center">
          <p className="text-sm text-slate-500">Failed to load session</p>
          <p className="text-xs text-slate-400 mt-1">{error?.message ?? "Unknown error"}</p>
          <button
            type="button"
            onClick={() => navigate(ROUTES.home)}
            className="mt-4 h-8 px-4 text-xs font-medium text-slate-600 bg-white border border-slate-200 rounded-lg hover:bg-slate-50"
          >
            Back to Home
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full bg-[#fafafa]">
      {/* Chat Header */}
      <header className="flex-shrink-0 h-14 flex items-center justify-between px-5 bg-white/80 backdrop-blur-sm border-b border-slate-200/80">
        <div className="flex items-center gap-3">
          <Link
            to={session ? ROUTES.workspace(session.workspace_id) : ROUTES.home}
            className="w-8 h-8 flex items-center justify-center rounded-lg text-slate-400 hover:text-slate-600 hover:bg-slate-100 transition-all duration-150 group"
          >
            <svg
              className="w-[18px] h-[18px] group-hover:-translate-x-0.5 transition-transform"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M15 19l-7-7 7-7" />
            </svg>
          </Link>
          <h1 className="text-sm font-semibold text-slate-900">Session</h1>
          {session && <StatusBadge status={session.status} />}
          {session?.model && (
            <span className="text-xs font-mono text-slate-400 ml-1">{session.model}</span>
          )}
        </div>
        <div className="flex items-center gap-2">
          {(session?.status === "connecting" || session?.status === "ready") && (
            <button
              type="button"
              onClick={() => cancelSession()}
              disabled={isCancelling}
              className="h-8 px-3.5 text-xs font-medium text-slate-600 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 hover:border-slate-300 transition-all duration-150 disabled:opacity-50"
            >
              Cancel
            </button>
          )}
        </div>
      </header>

      {/* Messages Area */}
      <div ref={scrollContainerRef} className="flex-1 overflow-y-auto">
        <div className="max-w-3xl mx-auto px-5 py-6 space-y-6">
          {messages.length === 0 && !liveBuffer.isStreaming && (
            <div className="flex items-center justify-center py-20">
              <p className="text-sm text-slate-400">Send a message to start the session</p>
            </div>
          )}

          {messages.map((msg) =>
            msg.role === "user" ? (
              <UserMessage key={msg.id} message={msg} />
            ) : (
              <AssistantMessage key={msg.id} message={msg} traces={traceMap.get(msg.id) ?? []} />
            ),
          )}

          <LiveAssistantMessage liveBuffer={liveBuffer} isStreaming={liveBuffer.isStreaming} />

          {/* Scroll target */}
          <div ref={scrollTargetRef} className="h-1" />
        </div>
      </div>

      {/* Input Area */}
      <MessageInput onSend={handleSend} disabled={isTerminal} isSending={isSending} />
    </div>
  );
}
