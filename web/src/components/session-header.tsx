import { Link } from "react-router";
import type { Provider, ResumeState, Session } from "../lib/types";
import { ROUTES } from "../lib/routes";
import { StatusBadge } from "./status-badge";

// ---------------------------------------------------------------------------
// SessionHeader — extracted from `pages/session.tsx` (feat-054)
//
// Owns the top bar of the session chat (back link, title, status badge,
// model, cwd, cancel button) and, for sessions in `wrapped` mode, a pill
// row showing the Runtime Tool display name, the active permission mode,
// and the most recent resume state. The native layout renders the bar
// unchanged — no pills, no behavior delta. Attended is reserved for
// Phase 11 and is treated as native defensively (no pills, no error).
// ---------------------------------------------------------------------------

/// Renders a single pill. The three runtime pills (runtime / permission
/// / resume) all share the same shape but want different colors; this
/// keeps the styling co-located with the markup.
function Pill({ label, className, title }: { label: string; className: string; title?: string }) {
  return (
    <span
      title={title}
      className={`inline-flex items-center px-2 py-0.5 rounded-md text-[10px] font-mono font-semibold tracking-wide border ${className}`}
    >
      {label}
    </span>
  );
}

const RUNTIME_KIND_LABEL: Record<Session["runtime_kind"], string> = {
  "anthropic-api": "Anthropic API",
  "openai-api": "OpenAI API",
  "openai-compatible": "OpenAI Compat",
  "claude-code": "Claude Code",
  codex: "Codex",
  opencode: "OpenCode",
};

const RESUME_STATE_PILL: Record<ResumeState, { label: string; className: string }> = {
  none: {
    label: "Resume: none",
    className: "bg-slate-100 text-slate-600 border-slate-200/60",
  },
  native: {
    label: "Resume: native",
    className: "bg-brand-emerald-50 text-brand-emerald-700 border-brand-emerald-200/60",
  },
  replayed: {
    label: "Resume: replayed",
    className: "bg-brand-amber-50 text-brand-amber-700 border-brand-amber-200/60",
  },
};

interface SessionHeaderProps {
  session: Session;
  /// Resolved via `useProviders()` on the page. `null` while the
  /// providers query is loading — the pill row degrades gracefully
  /// (the runtime pill falls back to the runtime kind enum name).
  provider: Provider | null;
  /// Last-wins per-turn resume outcome, mirrored from
  /// `useSession`'s live buffer. `null` before the first
  /// `message_persisted` / `done` of the turn (or for HTTP runtimes
  /// that never have a stored CLI resume id).
  resumeState: ResumeState | null;
  isCancelling: boolean;
  onCancel: () => void;
}

export function SessionHeader({
  session,
  provider,
  resumeState,
  isCancelling,
  onCancel,
}: SessionHeaderProps) {
  const isWrapped = session.mode === "wrapped";
  // Defensive default: attended and any future unknown mode render
  // as native. The server rejects attended at create time, so this
  // is the "shouldn't happen" fallback.
  const showPills = isWrapped;

  return (
    <header className="flex-shrink-0 h-14 flex items-center justify-between px-5 bg-white/80 backdrop-blur-sm border-b border-slate-200/80">
      <div className="flex items-center gap-3 min-w-0">
        <Link
          to={ROUTES.workspace(session.workspace_id)}
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
        <StatusBadge status={session.status} />
        {session.model && (
          <span className="text-xs font-mono text-slate-400 ml-1">{session.model}</span>
        )}
        {session.cwd && (
          <span
            title={session.cwd}
            className="text-[11px] font-mono text-slate-500 bg-slate-50 border border-slate-200/60 rounded-md px-1.5 py-0.5 max-w-[18rem] truncate"
          >
            {session.cwd.split("/").filter(Boolean).pop() || session.cwd}
          </span>
        )}
        {showPills && (
          <div className="flex items-center gap-1.5 ml-1" data-testid="wrapped-pill-row">
            <Pill
              label={provider?.name ?? RUNTIME_KIND_LABEL[session.runtime_kind]}
              title="Runtime Tool display name"
              className="bg-brand-orchid-50 text-brand-orchid-600 border-brand-orchid-200/60"
            />
            {provider?.permission_mode && (
              <Pill
                label={`Permissions: ${provider.permission_mode}`}
                title="Active permission mode"
                className="bg-slate-100 text-slate-600 border-slate-200/60"
              />
            )}
            {resumeState && (
              <Pill
                label={RESUME_STATE_PILL[resumeState].label}
                title="Per-turn resume state"
                className={RESUME_STATE_PILL[resumeState].className}
              />
            )}
          </div>
        )}
      </div>
      <div className="flex items-center gap-2 flex-shrink-0">
        {(session.status === "connecting" || session.status === "ready") && (
          <button
            type="button"
            onClick={onCancel}
            disabled={isCancelling}
            className="h-8 px-3.5 text-xs font-medium text-slate-600 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 hover:border-slate-300 transition-all duration-150 disabled:opacity-50"
          >
            Cancel
          </button>
        )}
      </div>
    </header>
  );
}
