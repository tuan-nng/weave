// AddCardModal — small form to create a new card in a specific column.
// Uses the shared `Modal` component for the backdrop / close behavior.
// The modal returns `{title, description?, codebase_id?}`; the parent
// (BoardContainer) injects the column_id before calling
// `api.kanban.cards.create`.
//
// F-15: Add a Codebase dropdown so the operator can pin the new
// card to a specific codebase at creation time. The dropdown is the
// same `useCodebases` list the TaskDetailPanel already uses (F-4),
// so the UX is consistent and the two views can't disagree about
// what codebases exist. Empty / "None" = no card-level binding, and
// the column's own binding (if any) wins for session spawning.

import { useState } from "react";
import { useCodebases } from "../../../hooks/use-codebase";
import { Modal } from "../../../components/modal";

export interface AddCardDraft {
  title: string;
  description?: string;
  /// F-15: optional card-level codebase binding. `null` =
  /// "no binding" (server omits the field, column binding wins).
  codebase_id?: string | null;
}

interface AddCardModalProps {
  open: boolean;
  workspaceId: string;
  onClose: () => void;
  onSubmit: (data: AddCardDraft) => void;
  isSubmitting: boolean;
}

export function AddCardModal({
  open,
  workspaceId,
  onClose,
  onSubmit,
  isSubmitting,
}: AddCardModalProps) {
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  /// F-15: `""` = "no binding". The form only sends a non-null
  /// id; the API treats null and absent as the same shape.
  const [codebaseId, setCodebaseId] = useState<string>("");
  /// F-15: fetch the workspace's codebases so the user can pick one.
  const { data: codebases = [] } = useCodebases(workspaceId);

  function handleClose() {
    if (isSubmitting) return;
    setTitle("");
    setDescription("");
    setCodebaseId("");
    onClose();
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmedTitle = title.trim();
    if (trimmedTitle.length === 0) return;
    onSubmit({
      title: trimmedTitle,
      description: description.trim() || undefined,
      codebase_id: codebaseId === "" ? null : codebaseId,
    });
    setTitle("");
    setDescription("");
    setCodebaseId("");
  }

  return (
    <Modal open={open} onClose={handleClose}>
      <form onSubmit={handleSubmit} className="space-y-4">
        <div className="flex items-center justify-between mb-6">
          <h3 className="text-lg font-semibold text-slate-900">Add card</h3>
          <button
            type="button"
            onClick={handleClose}
            className="text-slate-400 hover:text-slate-600 transition-colors"
            aria-label="Close"
          >
            <svg
              width="18"
              height="18"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={2}
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
        </div>

        <div>
          <label className="block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5">
            Title <span className="text-brand-red-500">*</span>
          </label>
          <input
            type="text"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="What needs to be done?"
            autoFocus
            className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
          />
        </div>

        <div>
          <label className="block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5">
            Description
          </label>
          <textarea
            rows={3}
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="Optional context, links, or acceptance criteria…"
            className="w-full px-3.5 py-2 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150 resize-none"
          />
        </div>

        {/* F-15: Codebase dropdown. Same data source as the
            TaskDetailPanel F-4 dropdown so the two views can't drift.
            "None" preserves the column-level binding; choosing a
            specific codebase pins the card for session spawning. */}
        <div>
          <label className="block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5">
            Codebase
          </label>
          <select
            value={codebaseId}
            onChange={(e) => setCodebaseId(e.target.value)}
            className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
          >
            <option value="">None (use column binding)</option>
            {codebases.map((cb) => (
              <option key={cb.id} value={cb.id}>
                {cb.label ?? cb.path}
                {cb.label ? ` — ${cb.path}` : ""}
              </option>
            ))}
          </select>
        </div>

        <div className="flex items-center justify-end gap-2 pt-2">
          <button
            type="button"
            onClick={handleClose}
            disabled={isSubmitting}
            className="h-9 px-3 text-sm font-medium text-slate-700 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 transition-all duration-150 disabled:opacity-50"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={title.trim().length === 0 || isSubmitting}
            className="h-9 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-all duration-150 disabled:opacity-50"
          >
            {isSubmitting ? "Adding…" : "Add card"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
