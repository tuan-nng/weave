import type { SessionStatus } from "../lib/types";

const STATUS_CONFIG: Record<
  SessionStatus,
  { bg: string; text: string; dot: string; border: string; label: string }
> = {
  connecting: {
    bg: "bg-brand-amber-50",
    text: "text-brand-amber-700",
    dot: "bg-brand-amber-500",
    border: "border-brand-amber-200/60",
    label: "Connecting",
  },
  ready: {
    bg: "bg-brand-emerald-50",
    text: "text-brand-emerald-700",
    dot: "bg-brand-emerald-500",
    border: "border-brand-emerald-200/60",
    label: "Ready",
  },
  completed: {
    bg: "bg-brand-blue-50",
    text: "text-brand-blue-700",
    dot: "bg-brand-blue-500",
    border: "border-brand-blue-200/60",
    label: "Completed",
  },
  error: {
    bg: "bg-brand-red-50",
    text: "text-brand-red-700",
    dot: "bg-brand-red-500",
    border: "border-brand-red-200/60",
    label: "Error",
  },
  cancelled: {
    bg: "bg-brand-slate-100",
    text: "text-brand-slate-600",
    dot: "bg-brand-slate-400",
    border: "border-brand-slate-200/60",
    label: "Cancelled",
  },
};

interface StatusBadgeProps {
  status: SessionStatus;
}

export function StatusBadge({ status }: StatusBadgeProps) {
  const config = STATUS_CONFIG[status] ?? STATUS_CONFIG.cancelled;

  return (
    <span
      className={`inline-flex items-center gap-1.5 px-2.5 py-1 ${config.bg} ${config.text} text-[11px] font-medium rounded-lg border ${config.border}`}
    >
      <span className={`w-1.5 h-1.5 rounded-full ${config.dot}`} />
      {config.label}
    </span>
  );
}
