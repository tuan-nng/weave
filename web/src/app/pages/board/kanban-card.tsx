// Kanban card — a single draggable task card. Uses @dnd-kit's useSortable
// so the card can be dragged within and across columns. The drag handle
// is the card itself; no separate handle element (the Open Design
// mockup's 6-dot grip is a hover-only affordance, not a separate drop
// target).

import { useSortable } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { Link } from "react-router";
import { ROUTES } from "../../../lib/routes";
import type { Task } from "../../../lib/types";
import { TaskStatusChip } from "./task-status-chip";

interface KanbanCardProps {
  task: Task;
  onClick: (task: Task) => void;
}

export function KanbanCard({ task, onClick }: KanbanCardProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: task.id,
  });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
  };

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
      className="group rounded-xl border border-slate-200/60 bg-white px-3 py-2.5 cursor-grab active:cursor-grabbing hover:border-slate-300/80 transition-colors"
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
          <Link
            to={ROUTES.session(task.session_id)}
            onClick={(e) => e.stopPropagation()}
            onPointerDown={(e) => e.stopPropagation()}
            className="inline-flex items-center gap-1 bg-brand-amber-50 text-brand-amber-700 border border-brand-amber-200/60 rounded-md px-1.5 py-0.5 text-[10px] font-medium hover:bg-brand-amber-100 transition-colors"
            title={`Open session ${task.session_id}`}
          >
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
            Agent
          </Link>
        )}
      </div>
    </div>
  );
}
