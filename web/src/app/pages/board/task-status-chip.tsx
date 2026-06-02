// Task status chip — visual chip for a Task's `status` field.
// Mirrors the shape of `StatusBadge` (session.tsx) but keyed to the 3
// kanban-level statuses (active/done/archived) instead of SessionStatus.

import type { TaskStatus } from "../../../lib/types";

const CONFIG: Record<
  TaskStatus,
  { dot: string; bg: string; text: string; border: string; label: string }
> = {
  active: {
    dot: "bg-brand-emerald-500",
    bg: "bg-brand-emerald-50",
    text: "text-brand-emerald-700",
    border: "border-brand-emerald-200/60",
    label: "active",
  },
  done: {
    dot: "bg-brand-blue-500",
    bg: "bg-brand-blue-50",
    text: "text-brand-blue-700",
    border: "border-brand-blue-200/60",
    label: "done",
  },
  archived: {
    dot: "bg-slate-400",
    bg: "bg-brand-slate-50",
    text: "text-brand-slate-600",
    border: "border-slate-200",
    label: "archived",
  },
};

export function TaskStatusChip({ status }: { status: string }) {
  // Defensive: server may one day introduce a new status; render a
  // neutral chip rather than crashing.
  const config = CONFIG[status as TaskStatus] ?? {
    dot: "bg-slate-300",
    bg: "bg-slate-50",
    text: "text-slate-600",
    border: "border-slate-200",
    label: status,
  };
  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-md px-1.5 py-0.5 text-[10px] font-medium border ${config.bg} ${config.text} ${config.border}`}
    >
      <span className={`w-1.5 h-1.5 rounded-full ${config.dot}`} />
      {config.label}
    </span>
  );
}
