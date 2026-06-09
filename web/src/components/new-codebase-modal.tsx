// NewCodebaseModal — shared "New Codebase" form bound to a single workspace.
// Used by:
//   - web/src/app/pages/codebases.tsx (per-workspace trigger on the list page)
//
// Open/close control: parent owns `useState<string | null>(null)` and passes
// `workspaceId={id-or-null}`. Matches the NewSessionModal / CreateBoardModal
// precedent. Form state, the useCreateCodebase mutation, and the form reset
// are owned by this component. Error is rendered inline so the modal is
// fully self-contained — no parent-level banner required. The optional
// `onCreated` callback fires AFTER the modal has closed on a successful
// create; the caller decides what to do (typically: navigate to the new
// codebase's detail page).

import { useEffect, useState } from "react";
import { Modal } from "./modal";
import { useCreateCodebase } from "../hooks/use-codebase";
import type { Codebase, CreateCodebaseRequest } from "../lib/types";

interface NewCodebaseModalProps {
  /** null = closed; non-null = open and bound to this workspace. */
  workspaceId: string | null;
  onClose: () => void;
  /**
   * Stack order. Default: 50. When the NewCodebaseModal is rendered
   * INSIDE another modal (e.g. NewSessionModal) the caller should pass
   * a higher value (e.g. 60) so it visually sits on top of the
   * outer's backdrop.
   */
  zIndex?: number;
  /** Fires AFTER the modal has closed on a successful create. */
  onCreated?: (codebase: Codebase) => void;
}

const FIELD_CLASS =
  "w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150";
const LABEL_CLASS =
  "block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5";

export function NewCodebaseModal({
  workspaceId,
  onClose,
  zIndex,
  onCreated,
}: NewCodebaseModalProps) {
  // useCreateCodebase is bound to a single workspace. We always call the
  // hook (rules of hooks) but pass "" while closed — the mutation is
  // only fired from inside handleSubmit, which re-checks workspaceId,
  // so the empty-string case is unreachable in practice.
  const createCodebase = useCreateCodebase(workspaceId ?? "");

  const [path, setPath] = useState("");
  const [label, setLabel] = useState("");
  const [error, setError] = useState<string | null>(null);

  // Reset form + error on every open transition (mount, or null→id, or
  // id1→id2). Does not fire on close (id→null).
  useEffect(() => {
    if (workspaceId !== null) {
      setPath("");
      setLabel("");
      setError(null);
    }
  }, [workspaceId]);

  function handleClose() {
    if (createCodebase.isPending) return;
    onClose();
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!workspaceId) return;
    const trimmedPath = path.trim();
    if (trimmedPath.length === 0) {
      setError("Path is required");
      return;
    }
    const payload: CreateCodebaseRequest = {
      path: trimmedPath,
      label: label.trim() === "" ? undefined : label.trim(),
    };
    createCodebase.mutate(payload, {
      onSuccess: (codebase) => {
        onClose();
        onCreated?.(codebase);
      },
      onError: (err) => {
        setError(err instanceof Error ? err.message : "Failed to create codebase");
      },
    });
  }

  return (
    <Modal open={workspaceId !== null} onClose={handleClose} zIndex={zIndex}>
      <form onSubmit={handleSubmit} className="space-y-4">
        <div className="flex items-center justify-between mb-6">
          <h3 className="text-lg font-semibold text-slate-900">New Codebase</h3>
          <button
            type="button"
            onClick={handleClose}
            aria-label="Close"
            className="text-slate-400 hover:text-slate-600 transition-colors"
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

        {error && (
          <div
            role="alert"
            className="rounded-lg border border-brand-red-200/60 bg-brand-red-50 px-3 py-2 text-xs text-brand-red-700"
          >
            {error}
          </div>
        )}

        {/* Path */}
        <div>
          <label className={LABEL_CLASS}>
            Path <span className="text-brand-red-500">*</span>
          </label>
          <input
            type="text"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="/Users/me/projects/my-app"
            autoFocus
            className={`${FIELD_CLASS} font-mono`}
          />
          <p className="mt-1.5 text-[11px] text-slate-500">
            Absolute path to a git working tree on disk.
          </p>
        </div>

        {/* Label */}
        <div>
          <label className={LABEL_CLASS}>Label</label>
          <input
            type="text"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="e.g. Backend, Mobile"
            className={FIELD_CLASS}
          />
        </div>

        {/* Actions */}
        <div className="flex justify-end gap-3 pt-2">
          <button
            type="button"
            onClick={handleClose}
            disabled={createCodebase.isPending}
            className="h-10 px-4 text-sm font-medium text-slate-700 bg-white border border-slate-200 rounded-xl hover:bg-slate-50 transition-all duration-150"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={createCodebase.isPending || path.trim().length === 0}
            className="h-10 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-all duration-150 disabled:opacity-50"
          >
            {createCodebase.isPending ? "Creating…" : "Create Codebase"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
