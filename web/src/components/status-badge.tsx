import type { SessionStatus } from "../lib/types";

const STATUS_CONFIG: Record<
  SessionStatus,
  { bg: string; text: string; dot: string; label: string }
> = {
  connecting: {
    bg: "bg-yellow-50",
    text: "text-yellow-700",
    dot: "bg-yellow-500",
    label: "Connecting",
  },
  ready: {
    bg: "bg-green-50",
    text: "text-green-700",
    dot: "bg-green-500",
    label: "Ready",
  },
  completed: {
    bg: "bg-blue-50",
    text: "text-blue-700",
    dot: "bg-blue-500",
    label: "Completed",
  },
  error: {
    bg: "bg-red-50",
    text: "text-red-700",
    dot: "bg-red-500",
    label: "Error",
  },
  cancelled: {
    bg: "bg-neutral-100",
    text: "text-neutral-600",
    dot: "bg-neutral-400",
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
      className={`inline-flex items-center gap-1.5 px-2.5 py-1 ${config.bg} ${config.text} text-xs font-medium rounded-md`}
    >
      <span className={`w-1.5 h-1.5 rounded-full ${config.dot}`} />
      {config.label}
    </span>
  );
}
