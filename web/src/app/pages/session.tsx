import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useSession } from "../../hooks/use-session";
import { ROUTES } from "../../lib/routes";
import { StatusBadge } from "../../components/status-badge";
import type { LiveBuffer, LiveThinkingBlock } from "../../hooks/use-session";
import type { Message, MessageMetadata, TraceRow } from "../../lib/types";

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

/**
 * Parse `messages.metadata` (TEXT NULL in the DB) and return the
 * stop_reason tag, if any. Invalid JSON is treated as no tag — a
 * legacy row written before the `message_persisted` protocol.
 */
function parseMessageMetadata(raw: string | null): MessageMetadata | null {
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (parsed && typeof parsed === "object" && "stop_reason" in parsed) {
      const sr = (parsed as { stop_reason: unknown }).stop_reason;
      if (sr === "cancelled" || sr === "error" || sr === "max_tokens") {
        return { stop_reason: sr };
      }
    }
    return null;
  } catch {
    return null;
  }
}

/**
 * Small chip shown next to an assistant message when its metadata
 * indicates an abnormal termination. Visible inline, not as a
 * full-width banner — the user has already seen the partial text,
 * so the chip is a quiet annotation.
 */
function StopReasonBadge({ stopReason }: { stopReason: MessageMetadata["stop_reason"] }) {
  if (!stopReason) return null;
  const config: Record<
    NonNullable<MessageMetadata["stop_reason"]>,
    { label: string; className: string }
  > = {
    cancelled: {
      label: "Cancelled",
      className: "bg-amber-50 text-amber-700 border border-amber-200/60",
    },
    error: {
      label: "Stopped (error)",
      className: "bg-rose-50 text-rose-700 border border-rose-200/60",
    },
    max_tokens: {
      label: "Stopped (max tokens)",
      className: "bg-slate-100 text-slate-600 border border-slate-200/60",
    },
  };
  const c = config[stopReason];
  return (
    <span
      className={`inline-flex items-center px-1.5 py-0.5 rounded-md text-[10px] font-medium ${c.className}`}
    >
      {c.label}
    </span>
  );
}

/**
 * Collapsible thinking block, default closed. The reducer accumulates
 * all thinking text for a turn into a single block; this component
 * owns the local "expanded" state. The block-level `expanded` flag on
 * `LiveThinkingBlock` is the initial value, so an in-flight thinking
 * block that the user has expanded stays expanded as more text
 * arrives (the reducer is responsible for preserving that).
 */
function ThinkingBlock({ block }: { block: LiveThinkingBlock }) {
  const [expanded, setExpanded] = useState(block.expanded);
  return (
    <div className="mb-3 rounded-lg border border-slate-200/80 bg-slate-50/60 overflow-hidden">
      <button
        type="button"
        aria-expanded={expanded}
        onClick={() => setExpanded(!expanded)}
        className="w-full flex items-center gap-2 px-3 py-2 text-left text-[11px] font-medium text-slate-500 hover:bg-slate-100/60 transition-colors"
      >
        <svg
          className="w-3 h-3 text-slate-400 transition-transform"
          style={{ transform: expanded ? "rotate(90deg)" : "rotate(0deg)" }}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M9 5l7 7-7 7" />
        </svg>
        <span>Thinking</span>
      </button>
      {expanded && (
        <div className="px-3 pb-3 text-[12px] text-slate-500 italic font-mono whitespace-pre-wrap border-t border-slate-200/60">
          {block.text}
        </div>
      )}
    </div>
  );
}

function AssistantMessage({
  message,
  traces,
  skipAnimate,
}: {
  message: Message;
  traces: TraceRow[];
  /**
   * When true, suppress the `animate-fade-in-up` mount animation.
   *
   * The animation runs on every fresh mount, including the moment
   * the persisted `AssistantMessage` replaces the live streaming
   * bubble. With the animation, the user sees: live bubble
   * disappears → blank → message fades in from below. That looks
   * like a reload of the same content.
   *
   * Skipping the animation makes the swap atomic from the user's
   * perspective: live bubble is gone, persisted bubble is there,
   * no intermediate fade. We only skip the animation for the
   * specific message that just replaced a live bubble — every
   * other mount (e.g. navigating to a session, loading history on
   * first paint) still gets the fade-in.
   */
  skipAnimate?: boolean;
}) {
  const time = new Date(message.created_at).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
  const meta = parseMessageMetadata(message.metadata);

  return (
    <div className={skipAnimate ? "flex justify-start" : "flex justify-start animate-fade-in-up"}>
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
          {meta?.stop_reason && <StopReasonBadge stopReason={meta.stop_reason} />}
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
  persistedMessage,
}: {
  liveBuffer: LiveBuffer;
  /**
   * The persisted Message keyed by `liveBuffer.persistedTurnId`, if
   * the history query has already returned it. We pass this in
   * rather than the full `messages` array so the component stays
   * pure and the parent can decide where the lookup lives.
   *
   * The live bubble hides only when this is defined — i.e. when the
   * persisted `AssistantMessage` is actually about to render. Hiding
   * on `liveBuffer.persistedTurnId` alone (the previous behavior)
   * created a visible gap: the live bubble would disappear the
   * moment the `message_persisted` SSE event landed, but the
   * `AssistantMessage` wouldn't render until the history refetch
   * resolved, which is one network round-trip later. The user saw:
   * streamed text → blank → re-rendered text. Gating on
   * `persistedMessage` closes the gap.
   */
  persistedMessage: Message | undefined;
}) {
  const text = liveBuffer.textChunks.join("");
  const toolCalls = Array.from(liveBuffer.toolCalls.values());
  const thinking = liveBuffer.thinking;

  // Id-based handoff: the server tells us the persisted row id via
  // `message_persisted`. The handoff completes when the matching row
  // is in the messages array (rendered as `AssistantMessage` by the
  // parent). The previous gate — "is persistedTurnId set?" — hid the
  // live bubble one network round-trip too early and produced a
  // visible flash. The new gate is "is the persisted row rendered?"
  // which is atomic with the persisted bubble appearing, so the user
  // sees a seamless swap.
  const liveSuperseded = persistedMessage !== undefined;
  const hasContent = text.length > 0 || toolCalls.length > 0 || thinking.length > 0;
  if (liveSuperseded || !hasContent) {
    return null;
  }

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
          {liveBuffer.isStreaming && (
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
          {/* Thinking blocks (collapsible) */}
          {thinking.map((block, i) => (
            <ThinkingBlock key={i} block={block} />
          ))}
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
          {liveBuffer.isStreaming && text.length === 0 && toolCalls.length === 0 && (
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
    pendingPrompts,
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
  // "↓ jump to latest" pill — shown when the user has scrolled up
  // while content is still arriving (live streaming or a refetch in
  // flight). Clicking the pill scrolls to the bottom and clears the
  // pill. Auto-hide when the user scrolls back near the bottom.
  const [showJumpPill, setShowJumpPill] = useState(false);

  const scrollToBottom = useCallback(() => {
    scrollTargetRef.current?.scrollIntoView({ behavior: "smooth" });
    setShowJumpPill(false);
  }, []);

  // Track scroll position
  useEffect(() => {
    const container = scrollContainerRef.current;
    if (!container) return;

    const handleScroll = () => {
      const { scrollTop, scrollHeight, clientHeight } = container;
      const atBottom = scrollHeight - scrollTop - clientHeight < 100;
      isAtBottomRef.current = atBottom;
      if (atBottom) setShowJumpPill(false);
    };

    container.addEventListener("scroll", handleScroll, { passive: true });
    return () => container.removeEventListener("scroll", handleScroll);
  }, []);

  // Auto-scroll on new content. The jump-pill appears when content
  // arrives while the user is scrolled up.
  const contentLength = messages.length + liveBuffer.textChunks.length;
  useEffect(() => {
    if (isAtBottomRef.current) {
      scrollToBottom();
    } else if (liveBuffer.isStreaming || liveBuffer.textChunks.length > 0) {
      setShowJumpPill(true);
    }
  }, [contentLength, scrollToBottom, liveBuffer.isStreaming, liveBuffer.textChunks.length]);

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

  // Live-bubble handoff: find the persisted message whose id matches
  // the most recent `message_persisted` event. Passing this into
  // `LiveAssistantMessage` closes the visible gap between "live
  // bubble disappears" and "persisted AssistantMessage renders" — the
  // previous version of the check (`persistedTurnId !== null`) hid
  // the live bubble one network round-trip too early.
  const livePersistedMessage = useMemo(
    () =>
      liveBuffer.persistedTurnId
        ? messages.find((m) => m.id === liveBuffer.persistedTurnId)
        : undefined,
    [liveBuffer.persistedTurnId, messages],
  );

  // Missing session ID guard (after hooks to satisfy rules-of-hooks)
  if (!id) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-sm text-slate-500">Missing session ID</p>
      </div>
    );
  }

  // Terminal state check — derived purely from the persisted session status.
  // `liveBuffer.stopReason` is intentionally NOT part of this check: a non-null
  // stopReason just records why the agent stopped (e.g. "end_turn", "max_tokens")
  // and does not mean the session is over. The backend keeps a successful session
  // in "ready" so the user can keep sending prompts in the same session.
  const isTerminal =
    session?.status === "completed" ||
    session?.status === "cancelled" ||
    session?.status === "error";

  // Loading — 3-row skeleton mirroring the message-list layout. This
  // eliminates the previous white-flash on first paint: the user sees
  // placeholder rows in the same shape as real messages, then they
  // fill in as the data lands.
  if (isLoading) {
    return (
      <div className="flex flex-col h-full bg-[#fafafa]">
        <div className="flex-1 overflow-y-auto">
          <div className="max-w-3xl mx-auto px-5 py-6 space-y-6">
            {/* User message skeleton (right-aligned) */}
            <div className="flex justify-end">
              <div className="max-w-[80%] h-10 w-64 rounded-2xl rounded-tr-md bg-slate-200/70 animate-pulse" />
            </div>
            {/* Assistant message skeleton (left-aligned) */}
            <div className="flex justify-start">
              <div className="w-full">
                <div className="h-4 w-24 rounded bg-slate-200/70 animate-pulse mb-2" />
                <div className="space-y-2">
                  <div className="h-3 w-full rounded bg-slate-200/70 animate-pulse" />
                  <div className="h-3 w-[90%] rounded bg-slate-200/70 animate-pulse" />
                  <div className="h-3 w-[75%] rounded bg-slate-200/70 animate-pulse" />
                </div>
              </div>
            </div>
            {/* Streaming indicator skeleton */}
            <div className="flex justify-start">
              <div className="w-full">
                <div className="h-4 w-24 rounded bg-slate-200/70 animate-pulse mb-2" />
                <div className="flex items-center gap-2 py-2">
                  <div className="flex items-center gap-1">
                    <div className="w-1.5 h-1.5 rounded-full bg-slate-300 animate-pulse" />
                    <div
                      className="w-1.5 h-1.5 rounded-full bg-slate-300 animate-pulse"
                      style={{ animationDelay: "0.15s" }}
                    />
                    <div
                      className="w-1.5 h-1.5 rounded-full bg-slate-300 animate-pulse"
                      style={{ animationDelay: "0.3s" }}
                    />
                  </div>
                  <div className="h-3 w-16 rounded bg-slate-200/70 animate-pulse" />
                </div>
              </div>
            </div>
          </div>
        </div>
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
              <AssistantMessage
                key={msg.id}
                message={msg}
                traces={traceMap.get(msg.id) ?? []}
                // The message that just replaced the live bubble
                // appears with the same content the user has been
                // watching. The fade-in animation is a 300ms
                // upward slide that, in this specific case, reads
                // as a "reload" of the same content. Suppress it
                // only for that one message; every other mount
                // (initial page load, navigating to a session)
                // still gets the fade-in.
                skipAnimate={msg.id === liveBuffer.persistedTurnId}
              />
            ),
          )}

          {/* Optimistic user prompts: sent but not yet in the persisted
              history. Rendered above the streaming assistant bubble so the
              user always sees their own message immediately on submit. The
              useSession hook drops a pending prompt from this list once the
              history query returns a message with the same id. */}
          {pendingPrompts.map((p) => (
            <UserMessage
              key={p.id}
              message={{
                id: p.id,
                session_id: sessionId,
                role: "user",
                content: p.content,
                metadata: null,
                created_at: p.createdAt,
              }}
            />
          ))}

          <LiveAssistantMessage liveBuffer={liveBuffer} persistedMessage={livePersistedMessage} />

          {/* Scroll target */}
          <div ref={scrollTargetRef} className="h-1" />
        </div>
      </div>

      {/* Input Area — wrapped in a relative container so the
          "↓ jump to latest" pill can float above it on the right. */}
      <div className="relative flex-shrink-0">
        {showJumpPill && (
          <button
            type="button"
            onClick={scrollToBottom}
            aria-label="Jump to latest message"
            className="absolute right-5 -top-9 z-10 inline-flex items-center gap-1.5 h-8 px-3 rounded-full bg-white border border-slate-200 shadow-md text-xs font-medium text-slate-600 hover:bg-slate-50 hover:border-slate-300 transition-colors animate-fade-in"
          >
            <svg
              className="w-3 h-3"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M19 14l-7 7m0 0l-7-7m7 7V3" />
            </svg>
            <span>Jump to latest</span>
          </button>
        )}
        <MessageInput onSend={handleSend} disabled={isTerminal} isSending={isSending} />
      </div>
    </div>
  );
}
