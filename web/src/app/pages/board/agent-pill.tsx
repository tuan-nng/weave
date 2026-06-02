// Specialist / auto-trigger chip on a column header. Renders the
// specialist name in a brand-amber pill (matching the established
// "agent / provider" semantic) and an amber dot if the column has
// auto_trigger enabled.

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
