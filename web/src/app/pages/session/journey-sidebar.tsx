import { useEffect, useRef, useState } from "react";
import { useJourney } from "../../../hooks/use-journey";
import { useFileChanges } from "../../../hooks/use-file-changes";
import type { FileChangeSummary, TraceRow } from "../../../lib/types";

// ---------------------------------------------------------------------------
// Public component
// ---------------------------------------------------------------------------

interface JourneySidebarProps {
  sessionId: string;
  /**
   * Whether the panel section is visible. The 40px rail is always
   * rendered so the user can re-open the sidebar after collapsing it.
   * The parent (`SessionPage`) owns the toggle state and a button in
   * the chat header.
   */
  isOpen: boolean;
  /**
   * Toggle handler called by the rail button and the panel-header
   * close button. Owned by the parent so the state lives next to
   * the chat header's matching toggle.
   */
  onToggle: () => void;
}

/**
 * Right-hand sidebar for the session page. Shows two stacked sections
 * pulled from the trace endpoints built in feat-017:
 *
 * 1. **Decisions & Errors** — `useJourney(sessionId)` → filtered
 *    chronological list of `decision` and `error` events. Decision
 *    rows are expandable (click to reveal full text from
 *    `data_json`); error rows are not.
 * 2. **Files** — `useFileChanges(sessionId)` → deduplicated list of
 *    files touched, with one chip per distinct action and a touch
 *    count. Clicking a path copies it to the clipboard with a
 *    brief "Copied!" tooltip.
 *
 * Visual design follows `weave-feat-022-journey-sidebar/journey-sidebar.html`:
 * brand-orchid chips for decisions, brand-red for errors, slate/blue/
 * emerald/red chips for read/write/create/delete file actions. All
 * expansion and tooltip animations use Tailwind v4 transitions
 * driven by component-local state.
 */
export function JourneySidebar({ sessionId, isOpen, onToggle }: JourneySidebarProps) {
  const journey = useJourney(sessionId);
  const files = useFileChanges(sessionId);

  return (
    <aside
      className={`flex-shrink-0 flex border-l border-slate-200/80 bg-white transition-[width] duration-200 ease-out ${
        isOpen ? "w-[360px]" : "w-10"
      }`}
    >
      {/* Collapsed rail — always visible so the user can re-open
          after collapsing. The chart icon is the entry point; the
          panel header has a matching close (×) button. */}
      <div className="w-10 flex-shrink-0 border-r border-slate-200/60 flex flex-col items-center pt-3 gap-2">
        <RailToggleButton onToggle={onToggle} />
      </div>

      {/* Panel — only when open. The aside width animates from
          40px → 360px on expand, so the chat column visibly grows
          when the sidebar closes. `FileChangeItem` clears its
          tooltip timer on unmount, so conditional rendering is safe. */}
      {isOpen && (
        <div className="flex-1 flex flex-col min-w-0 overflow-hidden animate-fade-in">
          <PanelHeader onToggle={onToggle} />
          <div className="flex-1 overflow-y-auto">
            <JourneyTimeline
              events={journey.data ?? []}
              isLoading={journey.isLoading}
              isError={journey.isError}
            />
            <div className="mx-4 border-t border-slate-200/60" />
            <FileChangesList
              changes={files.data ?? []}
              isLoading={files.isLoading}
              isError={files.isError}
            />
          </div>
        </div>
      )}
    </aside>
  );
}

// ---------------------------------------------------------------------------
// Rail toggle button — entry point on the always-visible 40px rail.
// ---------------------------------------------------------------------------

interface RailToggleButtonProps {
  onToggle: () => void;
}

function RailToggleButton({ onToggle }: RailToggleButtonProps) {
  return (
    <button
      type="button"
      onClick={onToggle}
      // "Toggle" rather than show/hide — the rail button is always
      // visible and always toggles, regardless of current state.
      // Distinct from the panel close (×) button which is always
      // a "hide" affordance.
      title="Toggle Journey sidebar"
      aria-label="Toggle Journey sidebar"
      className="w-7 h-7 rounded-lg flex items-center justify-center text-slate-400 hover:text-slate-600 hover:bg-slate-100 transition-colors"
    >
      <svg
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M3 3v18h18" />
        <path d="M7 16l4-8 4 4 4-6" />
      </svg>
    </button>
  );
}

// ---------------------------------------------------------------------------
// Panel header — "Journey" title + close (×) button.
// ---------------------------------------------------------------------------

function PanelHeader({ onToggle }: { onToggle: () => void }) {
  return (
    <div className="h-14 flex-shrink-0 flex items-center justify-between px-4 border-b border-slate-200/80">
      <h2 className="text-sm font-display font-semibold text-slate-800">Journey</h2>
      <button
        type="button"
        onClick={onToggle}
        title="Hide Journey sidebar"
        aria-label="Hide Journey sidebar"
        className="w-6 h-6 rounded-md flex items-center justify-center text-slate-400 hover:text-slate-600 hover:bg-slate-100 transition-colors"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={2}
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <line x1="18" y1="6" x2="6" y2="18" />
          <line x1="6" y1="6" x2="18" y2="18" />
        </svg>
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// JourneyTimeline (Decisions + Errors)
// ---------------------------------------------------------------------------

interface JourneyTimelineProps {
  events: TraceRow[];
  isLoading: boolean;
  isError: boolean;
}

function JourneyTimeline({ events, isLoading, isError }: JourneyTimelineProps) {
  return (
    <section className="px-4 pt-4 pb-2">
      <div className="flex items-center gap-1.5 mb-3">
        <svg
          width="13"
          height="13"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={2}
          strokeLinecap="round"
          strokeLinejoin="round"
          className="text-slate-400"
        >
          <circle cx="12" cy="12" r="10" />
          <polyline points="12 6 12 12 16 14" />
        </svg>
        <span className="text-[11px] font-semibold text-slate-500 uppercase tracking-wider">
          Decisions &amp; Errors
        </span>
      </div>

      {isLoading && events.length === 0 ? (
        <SkeletonRows count={2} />
      ) : isError ? (
        <EmptyHint text="Failed to load journey" />
      ) : events.length === 0 ? (
        <EmptyHint text="No decisions or errors yet" />
      ) : (
        <div className="space-y-2">
          {events.map((event) =>
            event.event_type === "error" ? (
              <ErrorNode key={event.id} event={event} />
            ) : (
              <DecisionNode key={event.id} event={event} />
            ),
          )}
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// DecisionNode — expandable card for a `decision` event.
//
// Click anywhere on the card to toggle. The summary is the
// backend-truncated `summary`; the full text is parsed from
// `data_json` (the Rust store keeps both — the truncated version
// for list views, the JSON payload for "give me the whole thing").
// ---------------------------------------------------------------------------

interface DecisionNodeProps {
  event: TraceRow;
}

function DecisionNode({ event }: DecisionNodeProps) {
  const [expanded, setExpanded] = useState(false);
  const fullText = parseFullText(event);

  return (
    <div
      role="button"
      tabIndex={0}
      aria-expanded={expanded}
      onClick={() => setExpanded((v) => !v)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          setExpanded((v) => !v);
        }
      }}
      className="group rounded-xl border border-slate-200/60 bg-white hover:border-slate-200 transition-colors cursor-pointer"
    >
      <div className="flex items-start gap-2.5 px-3 py-2.5">
        <DecisionChip />
        <div className="flex-1 min-w-0">
          <p className="text-[11px] text-slate-700 leading-relaxed">{event.summary}</p>
          {fullText && (
            <div
              className={`overflow-hidden transition-all duration-200 ease-out ${
                expanded ? "max-h-[600px] overflow-y-auto opacity-100 mt-2" : "max-h-0 opacity-0"
              }`}
            >
              <p className="text-[11px] text-slate-500 leading-relaxed">{fullText}</p>
            </div>
          )}
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          <span className="text-[10px] text-slate-400 tabular-nums">
            {formatTime(event.timestamp)}
          </span>
          {fullText && (
            <svg
              className={`w-3 h-3 text-slate-400 transition-transform duration-200 ${
                expanded ? "rotate-90" : ""
              }`}
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={2}
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <polyline points="9 18 15 12 9 6" />
            </svg>
          )}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// ErrorNode — non-expandable card. Errors are short and important;
// no need to clutter the UI with a chevron.
// ---------------------------------------------------------------------------

function ErrorNode({ event }: DecisionNodeProps) {
  return (
    <div className="rounded-xl border border-brand-red-200/60 bg-brand-red-50/40 px-3 py-2.5">
      <div className="flex items-start gap-2.5">
        <ErrorChip />
        <div className="flex-1 min-w-0">
          <p className="text-[11px] text-brand-red-700 leading-relaxed">{event.summary}</p>
        </div>
        <span className="text-[10px] text-slate-400 flex-shrink-0 tabular-nums">
          {formatTime(event.timestamp)}
        </span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Chips (small visual badges for event types)
// ---------------------------------------------------------------------------

function DecisionChip() {
  return (
    <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md bg-brand-orchid-50 text-brand-orchid-600 text-[10px] font-medium flex-shrink-0 mt-0.5">
      <svg
        width="10"
        height="10"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={2.5}
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M9 18h6" />
        <path d="M10 22h4" />
        <path d="M15.09 14c.18-.98.65-1.74 1.41-2.5A4.65 4.65 0 0 0 18 8 6 6 0 0 0 6 8c0 1 .23 2.23 1.5 3.5A4.61 4.61 0 0 1 8.91 14" />
      </svg>
      Decision
    </span>
  );
}

function ErrorChip() {
  return (
    <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md bg-brand-red-100 text-brand-red-600 text-[10px] font-medium flex-shrink-0 mt-0.5">
      <svg
        width="10"
        height="10"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={2.5}
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
        <line x1="12" y1="9" x2="12" y2="13" />
        <line x1="12" y1="17" x2="12.01" y2="17" />
      </svg>
      Error
    </span>
  );
}

// ---------------------------------------------------------------------------
// FileChangesList
// ---------------------------------------------------------------------------

interface FileChangesListProps {
  changes: FileChangeSummary[];
  isLoading: boolean;
  isError: boolean;
}

function FileChangesList({ changes, isLoading, isError }: FileChangesListProps) {
  return (
    <section className="px-4 pt-4 pb-4">
      <div className="flex items-center gap-1.5 mb-3">
        <svg
          width="13"
          height="13"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth={2}
          strokeLinecap="round"
          strokeLinejoin="round"
          className="text-slate-400"
        >
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
          <polyline points="14 2 14 8 20 8" />
        </svg>
        <span className="text-[11px] font-semibold text-slate-500 uppercase tracking-wider">
          Files
        </span>
        {!isLoading && !isError && changes.length > 0 && (
          <span className="ml-auto text-[10px] text-slate-400">
            {changes.length} {changes.length === 1 ? "file" : "files"}
          </span>
        )}
      </div>

      {isLoading && changes.length === 0 ? (
        <SkeletonRows count={3} compact />
      ) : isError ? (
        <EmptyHint text="Failed to load file changes" />
      ) : changes.length === 0 ? (
        <EmptyHint text="No files touched yet" />
      ) : (
        <div className="space-y-1">
          {changes.map((change) => (
            <FileChangeItem key={change.path} change={change} />
          ))}
        </div>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------
// FileChangeItem — one row per file. Click copies the path to the
// clipboard and shows a "Copied!" tooltip for 1.2s. The tooltip
// state is component-local; the timer is cleared on unmount.
// ---------------------------------------------------------------------------

interface FileChangeItemProps {
  change: FileChangeSummary;
}

function FileChangeItem({ change }: FileChangeItemProps) {
  // `false` = no toast, `"ok"` = success, `"fail"` = clipboard
  // write rejected. The tri-state is what lets us show a distinct
  // message on failure without the toast lying about success.
  const [copied, setCopied] = useState<false | "ok" | "fail">(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Clear any pending timer if the component unmounts mid-tooltip.
  // Without this, React's "state update on unmounted component"
  // warning fires when the user navigates away.
  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
  }, []);

  const handleCopy = async () => {
    if (timerRef.current) clearTimeout(timerRef.current);
    // Try the copy; on failure, surface a distinct failure toast so
    // the user knows the path is NOT on their clipboard. Lying about
    // success is worse than not showing anything.
    let didCopy = false;
    try {
      await navigator.clipboard.writeText(change.path);
      didCopy = true;
    } catch {
      didCopy = false;
    }
    setCopied(didCopy ? "ok" : "fail");
    timerRef.current = setTimeout(() => setCopied(false), 1200);
  };

  const hasDelete = change.actions.includes("delete");

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={handleCopy}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          handleCopy();
        }
      }}
      title={`Copy ${change.path}`}
      className="relative flex items-center gap-2.5 px-2.5 py-2 rounded-lg hover:bg-slate-50 cursor-pointer transition-colors group"
    >
      <span
        className={`text-[11px] font-mono truncate flex-1 ${
          hasDelete ? "text-slate-500 line-through" : "text-slate-700"
        }`}
      >
        {change.path}
      </span>
      <div className="flex items-center gap-1 flex-shrink-0">
        {change.actions.map((action) => (
          <FileActionChip key={action} action={action} />
        ))}
      </div>
      <span className="text-[9px] text-slate-400 tabular-nums flex-shrink-0">×{change.count}</span>
      <span
        aria-hidden={copied === false}
        className={`pointer-events-none absolute right-3 -top-7 bg-slate-800 text-white text-[10px] px-2 py-1 rounded-md shadow-md transition-all duration-150 ${
          copied ? "opacity-100 translate-y-0" : "opacity-0 translate-y-1"
        }`}
      >
        {copied === "fail" ? "Copy failed" : "Copied!"}
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// FileActionChip — defensive: render any string from the API, with
// a sensible color map for the four known values and a neutral
// slate fallback for anything else (e.g. a future action variant).
// ---------------------------------------------------------------------------

const FILE_ACTION_CONFIG: Record<string, { bg: string; text: string; label: string }> = {
  read: { bg: "bg-brand-slate-100", text: "text-brand-slate-600", label: "read" },
  write: { bg: "bg-brand-blue-50", text: "text-brand-blue-600", label: "write" },
  create: { bg: "bg-brand-emerald-50", text: "text-brand-emerald-600", label: "create" },
  delete: { bg: "bg-brand-red-50", text: "text-brand-red-600", label: "delete" },
};

function FileActionChip({ action }: { action: string }) {
  const config = FILE_ACTION_CONFIG[action] ?? {
    bg: "bg-slate-100",
    text: "text-slate-500",
    label: action,
  };
  return (
    <span
      className={`inline-flex px-1.5 py-0.5 rounded text-[9px] font-medium ${config.bg} ${config.text}`}
    >
      {config.label}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Tiny shared UI bits
// ---------------------------------------------------------------------------

function SkeletonRows({ count, compact = false }: { count: number; compact?: boolean }) {
  return (
    <div className={compact ? "space-y-1" : "space-y-2"}>
      {Array.from({ length: count }).map((_, i) => (
        <div
          key={i}
          className={`rounded-xl border border-slate-200/60 bg-white px-3 ${
            compact ? "h-8" : "h-14"
          } animate-pulse`}
        />
      ))}
    </div>
  );
}

function EmptyHint({ text }: { text: string }) {
  return <p className="text-[11px] text-slate-400 italic px-1 py-2">{text}</p>;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Pull the full decision text from `data_json`. Returns null if the
 * field is missing or unparseable — in which case the row shows only
 * the truncated summary and the expand affordance is hidden (no point
 * in expanding a string that isn't there).
 *
 * Decisions always carry `{ text }` (per `TraceEventKind::Decision`
 * in `store/traces.rs`); errors carry `{ message }` and are handled
 * separately by `ErrorNode` which doesn't expand.
 */
function parseFullText(event: TraceRow): string | null {
  if (!event.data_json) return null;
  try {
    const data = JSON.parse(event.data_json) as Record<string, unknown>;
    const text = data.text;
    return typeof text === "string" && text.length > 0 ? text : null;
  } catch {
    return null;
  }
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return "";
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
