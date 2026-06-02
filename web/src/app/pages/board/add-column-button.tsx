// The rightmost "+ Add column" placeholder. Dashed outline, hover
// state matches the established design system.

interface AddColumnButtonProps {
  onClick: () => void;
}

export function AddColumnButton({ onClick }: AddColumnButtonProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="w-[280px] flex-shrink-0 h-32 rounded-2xl border-2 border-dashed border-slate-200/80 bg-white/40 flex items-center justify-center text-sm text-slate-400 hover:text-slate-600 hover:border-slate-300 transition-colors"
    >
      + Add column
    </button>
  );
}
