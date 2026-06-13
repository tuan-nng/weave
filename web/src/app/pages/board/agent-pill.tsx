// Specialist / auto-trigger chip on a column header. Renders the
// specialist name in a brand-amber pill (matching the established
// "agent / provider" semantic) and an amber dot if the column has
// auto_trigger enabled.

import type { ColumnStage, RuntimeKind } from "../../../lib/types";

const RUNTIME_KIND_LABEL: Record<RuntimeKind, string> = {
  "anthropic-api": "Anthropic API",
  "openai-api": "OpenAI API",
  "openai-compatible": "OpenAI Compat",
  "claude-code": "Claude Code",
  codex: "Codex",
  opencode: "OpenCode",
};

/// Short, single-word stage labels for the column header badge
/// (feat-068 F-3). The full "In Progress" / "Backlog" / etc. names
/// would overflow a 280px column header; the short labels preserve
/// the meaning in 6-8 chars.
const STAGE_SHORT_LABEL: Record<ColumnStage, string> = {
  backlog: "Backlog",
  todo: "Todo",
  dev: "In Prog",
  review: "Review",
  // "Done" matches the column-name label too, so we use
  // "Shipped" to keep the in-DOM text unique (a "Done" column
  // with a "Done" stage badge would render the same word twice
  // in 280px-wide headers, and `findByText` in tests would
  // match both). Wire value `done` is still rendered in the
  // Lane footer / prompt.
  done: "Shipped",
};

export function SpecialistChip({ name }: { name: string }) {
  return (
    <span className="inline-flex items-center gap-1 bg-brand-amber-50 text-brand-amber-700 border border-brand-amber-200/60 rounded-md px-1.5 py-0.5 text-[10px] font-medium">
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
      {name}
    </span>
  );
}

export function AutoTriggerDot() {
  return (
    <span className="w-1.5 h-1.5 rounded-full bg-brand-amber-500" title="Auto-trigger enabled" />
  );
}

export function RuntimeKindBadge({ kind }: { kind: RuntimeKind }) {
  return (
    <span className="inline-flex items-center gap-1 bg-slate-100 text-slate-600 border border-slate-200/60 rounded-md px-1.5 py-0.5 text-[10px] font-medium">
      {RUNTIME_KIND_LABEL[kind] ?? kind}
    </span>
  );
}

/// Per-column stage badge (feat-068 F-3). Renders the short
/// stage name in a slate pill. The orchestrator's prompt uses
/// the same `stage` value to pick the "advance to &lt;stage&gt;"
/// line, so the badge on the column header matches what the
/// agent will see.
export function StageBadge({ stage }: { stage: ColumnStage }) {
  return (
    <span
      className="inline-flex items-center bg-slate-50 text-slate-500 border border-slate-200/60 rounded-md px-1.5 py-0.5 text-[10px] font-medium"
      title={`Stage: ${stage}`}
    >
      {STAGE_SHORT_LABEL[stage] ?? stage}
    </span>
  );
}
