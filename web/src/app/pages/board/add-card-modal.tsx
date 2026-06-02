// AddCardModal — small form to create a new card in a specific column.
// Uses the shared `Modal` component for the backdrop / close behavior.
// The modal returns just `{title, description?}`; the parent (BoardContainer)
// injects the column_id before calling `api.kanban.cards.create`.

import { useState } from "react";
import { Modal } from "../../../components/modal";

export interface AddCardDraft {
  title: string;
  description?: string;
}

interface AddCardModalProps {
  open: boolean;
  onClose: () => void;
  onSubmit: (data: AddCardDraft) => void;
  isSubmitting: boolean;
}

export function AddCardModal({ open, onClose, onSubmit, isSubmitting }: AddCardModalProps) {
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");

  function handleClose() {
    if (isSubmitting) return;
    setTitle("");
    setDescription("");
    onClose();
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmedTitle = title.trim();
    if (trimmedTitle.length === 0) return;
    onSubmit({
      title: trimmedTitle,
      description: description.trim() || undefined,
    });
    setTitle("");
    setDescription("");
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
