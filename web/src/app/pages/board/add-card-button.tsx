// "+ Add card" tile rendered at the bottom of every column. Opens
// the AddCardModal (inline in BoardPage) for the given column.

interface AddCardButtonProps {
  onClick: () => void;
}

export function AddCardButton({ onClick }: AddCardButtonProps) {
  return (
    <div className="p-2 pt-0">
      <button
        type="button"
        onClick={onClick}
        className="w-full h-8 rounded-lg text-[12px] text-slate-400 hover:text-slate-600 hover:bg-slate-50 border border-dashed border-slate-200/60 transition-colors"
      >
        + Add card
      </button>
    </div>
  );
}
