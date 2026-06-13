// Kanban card — a single draggable task card. Uses @dnd-kit's useSortable
// so the card can be dragged within and across columns. The drag handle
// is the card itself; no separate handle element (the Open Design
// mockup's 6-dot grip is a hover-only affordance, not a separate drop
// target).
//
// F-14: the Agent pill now reflects the bound session's live state
// (running / needs input / errored / cancelled). The previous
// implementation rendered a static "Agent" badge with no way to
// tell whether the agent was actively working or waiting for the
// user to send the next prompt.

import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { Link } from "react-router";
import { describeSession, useAgentStatus } from "../../../hooks/use-agent-status";
import { ROUTES } from "../../../lib/routes";
import type { Task } from "../../../lib/types";
import { TaskStatusChip } from "./task-status-chip";

interface KanbanCardProps {
  task: Task;
  onClick: (task: Task) => void;
}

const TONE_CLASSES: Record<string, string> = {
  neutral: "bg-slate-50 text-slate-700 border-slate-200/60 hover:bg-slate-100",
  amber:
    "bg-brand-amber-50 text-brand-amber-700 border-brand-amber-200/60 hover:bg-brand-amber-100",
  rose: "bg-brand-red-50 text-brand-red-700 border-brand-red-200/60 hover:bg-brand-red-100",
  emerald:
    "bg-brand-emerald-50 text-brand-emerald-700 border-brand-emerald-200/60 hover:bg-brand-emerald-100",
  blue: "bg-brand-blue-50 text-brand-blue-700 border-brand-blue-200/60 hover:bg-brand-blue-100",
  slate: "bg-slate-100 text-slate-600 border-slate-200/60 hover:bg-slate-200/60",
};

export function KanbanCard({ task, onClick }: KanbanCardProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: task.id,
  });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
  };

  // F-14: only fetch session status for cards that have a bound session.
  // The query is short-circuited when session_id is null. The
  // returned `Session` carries `awaiting_user_input` + `status` from
  // the backend so the pill has all the data it needs from one round
  // trip.
  const agentQuery = useAgentStatus(task.session_id);
  // Until the query resolves, default to a neutral "Agent" pill so the
  // card layout is stable on first paint.
  const agentState = task.session_id
    ? describeSession(agentQuery.data ?? null)
    : { state: "idle" as const, status: null, label: "", tone: "amber" as const, hint: "" };
  const tone = task.session_id ? agentState.tone : "amber";
  const pillLabel = task.session_id ? agentState.label || "Agent" : "Agent";
  const pillHint = task.session_id ? agentState.hint : `Open session ${task.session_id}`;
  // The needs-input state should stand out: ring + dot.
  const isNeedsInput = agentState.state === "needs_input";

  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      {...listeners}
      onClick={(e) => {
        // Suppress click after drag — dnd-kit's PointerSensor doesn't
        // emit a click in that case, but be defensive.
        if (!isDragging) onClick(task);
        e.stopPropagation();
      }}
      className={`group rounded-xl border bg-white px-3 py-2.5 cursor-grab active:cursor-grabbing transition-colors ${
        isNeedsInput
          ? "border-brand-red-300 hover:border-brand-red-400 ring-1 ring-brand-red-200/60"
          : "border-slate-200/60 hover:border-slate-300/80"
      }`}
    >
      <div className="flex items-start justify-between gap-2">
        <span className="text-[13px] font-medium text-slate-900 leading-snug flex-1">
          {task.title}
        </span>
        {/* Drag handle — appears on hover. Same 6-dot grip as the
            Open Design mockup; inline SVG to match the no-icon-library
            convention. */}
        <svg
          className="w-3.5 h-3.5 text-slate-300 opacity-0 group-hover:opacity-100 transition-opacity flex-shrink-0 mt-0.5"
          viewBox="0 0 16 16"
          fill="currentColor"
          aria-hidden="true"
        >
          <circle cx="5" cy="3" r="1.5" />
          <circle cx="11" cy="3" r="1.5" />
          <circle cx="5" cy="8" r="1.5" />
          <circle cx="11" cy="8" r="1.5" />
          <circle cx="5" cy="13" r="1.5" />
          <circle cx="11" cy="13" r="1.5" />
        </svg>
      </div>
      <div className="flex items-center justify-between mt-2">
        <TaskStatusChip status={task.status} />
        {task.session_id && (
          // The agent indicator pill links to the session. Using a
          // Link (not a button) preserves middle-click-to-open-in-new-tab.
          // F-14: tone + label are derived from the bound session's
          // status; the static "Agent" fallback is shown until the
          // session fetch resolves (skeleton-avoidance).
          <Link
            to={ROUTES.session(task.session_id)}
            onClick={(e) => e.stopPropagation()}
            onPointerDown={(e) => e.stopPropagation()}
            className={`inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] font-medium border transition-colors ${TONE_CLASSES[tone] ?? TONE_CLASSES.amber}`}
            title={pillHint}
            aria-label={pillHint}
            data-agent-state={agentState.state}
          >
            {isNeedsInput && (
              <span className="w-1.5 h-1.5 rounded-full bg-brand-red-500" aria-hidden="true" />
            )}
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
              <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
            </svg>
            {pillLabel}
          </Link>
        )}
      </div>
    </div>
  );
}
