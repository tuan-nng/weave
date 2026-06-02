// AddColumnModal — small form to create a new column. Auto-trigger
// requires a specialist_id (per the backend's `validate_auto_trigger`
// guard at `crates/weave-server/src/store/columns.rs:181-188`), so the
// specialist selector is enabled only when the auto-trigger toggle is
// on. The specialists list comes from the existing
// `api.specialists.list()` endpoint.

import { useState } from "react";
import { Modal } from "../../../components/modal";
import { useSpecialists } from "../../../hooks/use-specialists";
import type { CreateColumnRequest } from "../../../lib/types";

interface AddColumnModalProps {
  open: boolean;
  onClose: () => void;
  onSubmit: (data: CreateColumnRequest) => void;
  isSubmitting: boolean;
}

export function AddColumnModal({ open, onClose, onSubmit, isSubmitting }: AddColumnModalProps) {
  const [name, setName] = useState("");
  const [autoTrigger, setAutoTrigger] = useState(false);
  const [specialistId, setSpecialistId] = useState("");

  // `useSpecialists` is a thin TanStack Query wrapper around
  // `api.specialists.list()`. Available because the session page
  // already uses it (web/src/hooks/use-specialists.ts).
  const { data: specialists = [] } = useSpecialists();

  function handleClose() {
    if (isSubmitting) return;
    setName("");
    setAutoTrigger(false);
    setSpecialistId("");
    onClose();
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = name.trim();
    if (trimmed.length === 0) return;
    if (autoTrigger && specialistId === "") return; // server would 400
    onSubmit({
      name: trimmed,
      auto_trigger: autoTrigger || undefined,
      specialist_id: autoTrigger ? specialistId : undefined,
    });
    setName("");
    setAutoTrigger(false);
    setSpecialistId("");
  }

  return (
    <Modal open={open} onClose={handleClose}>
      <form onSubmit={handleSubmit} className="space-y-4">
        <div className="flex items-center justify-between mb-6">
          <h3 className="text-lg font-semibold text-slate-900">Add column</h3>
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
            Name <span className="text-brand-red-500">*</span>
          </label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. In Review"
            autoFocus
            className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
          />
        </div>

        <label className="flex items-center gap-2 text-sm text-slate-700 cursor-pointer">
          <input
            type="checkbox"
            checked={autoTrigger}
            onChange={(e) => setAutoTrigger(e.target.checked)}
            className="w-4 h-4 rounded border-slate-300 text-brand-blue-500 focus:ring-brand-blue-500/30"
          />
          Auto-trigger a specialist when a card is moved here
        </label>

        {autoTrigger && (
          <div>
            <label className="block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5">
              Specialist <span className="text-brand-red-500">*</span>
            </label>
            <select
              value={specialistId}
              onChange={(e) => setSpecialistId(e.target.value)}
              className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
            >
              <option value="">Select a specialist…</option>
              {specialists.map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name}
                </option>
              ))}
            </select>
            <p className="text-[10px] text-slate-400 mt-1.5">
              Moving a card into this column will spawn a new session with the chosen specialist.
            </p>
          </div>
        )}

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
            disabled={
              name.trim().length === 0 || (autoTrigger && specialistId === "") || isSubmitting
            }
            className="h-9 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-all duration-150 disabled:opacity-50"
          >
            {isSubmitting ? "Adding…" : "Add column"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
