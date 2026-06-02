// TaskDetailPanel — slide-over editor for a single task. Reads the
// task from props (the parent passes the canonical `Task` from
// useBoard, so SSE patches propagate without an extra useQuery).
// Saves via the parent's updateTask callback.

import { useEffect, useState } from "react";
import { Link } from "react-router";
import { ROUTES } from "../../../lib/routes";
import type { Task, TaskStatus, UpdateTaskRequest } from "../../../lib/types";

interface TaskDetailPanelProps {
  task: Task | null;
  onClose: () => void;
  onSave: (taskId: string, data: UpdateTaskRequest) => void;
  onDelete: (taskId: string) => void;
  isSaving: boolean;
  isDeleting: boolean;
}

export function TaskDetailPanel({
  task,
  onClose,
  onSave,
  onDelete,
  isSaving,
  isDeleting,
}: TaskDetailPanelProps) {
  // Local form state. Initialized from the canonical `task` whenever
  // the panel opens or the user switches to a different card (keyed
  // by task.id via the useEffect dependency).
  const [draft, setDraft] = useState<{
    title: string;
    description: string;
    status: TaskStatus;
    acceptance_criteria: string;
    completion_summary: string;
    verification_report: string;
  }>(blankDraft);

  useEffect(() => {
    if (task) {
      setDraft({
        title: task.title,
        description: task.description ?? "",
        status: task.status as TaskStatus,
        acceptance_criteria: task.acceptance_criteria ?? "",
        completion_summary: task.completion_summary ?? "",
        verification_report: task.verification_report ?? "",
      });
    } else {
      setDraft(blankDraft);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task?.id]);

  const open = task !== null;
  // `hasChanges` — compare to the current canonical task. Disabled
  // Save when no edits OR title is empty.
  const hasChanges =
    !!task &&
    (draft.title !== task.title ||
      draft.description !== (task.description ?? "") ||
      draft.status !== task.status ||
      draft.acceptance_criteria !== (task.acceptance_criteria ?? "") ||
      draft.completion_summary !== (task.completion_summary ?? "") ||
      draft.verification_report !== (task.verification_report ?? ""));
  const canSave = hasChanges && draft.title.trim().length > 0 && !isSaving;

  function handleSave() {
    if (!task || !canSave) return;
    // Build the update DTO. For nullable fields, send `null` only if
    // the user explicitly cleared the field; if unchanged from the
    // current value (including null), omit the key to preserve the
    // server's tri-state "leave alone" semantics.
    const data: UpdateTaskRequest = {};
    if (draft.title !== task.title) data.title = draft.title;
    if (draft.description !== (task.description ?? "")) {
      data.description = draft.description === "" ? null : draft.description;
    }
    if (draft.status !== task.status) data.status = draft.status;
    if (draft.acceptance_criteria !== (task.acceptance_criteria ?? "")) {
      data.acceptance_criteria =
        draft.acceptance_criteria === "" ? null : draft.acceptance_criteria;
    }
    if (draft.completion_summary !== (task.completion_summary ?? "")) {
      data.completion_summary = draft.completion_summary === "" ? null : draft.completion_summary;
    }
    if (draft.verification_report !== (task.verification_report ?? "")) {
      data.verification_report =
        draft.verification_report === "" ? null : draft.verification_report;
    }
    onSave(task.id, data);
  }

  return (
    <>
      {/* Backdrop — click outside closes the panel */}
      <div
        onClick={onClose}
        className={`fixed inset-0 bg-black/30 z-30 transition-opacity duration-300 ${
          open ? "opacity-100 pointer-events-auto" : "opacity-0 pointer-events-none"
        }`}
        aria-hidden="true"
      />
      <aside
        role="dialog"
        aria-modal="true"
        aria-hidden={!open}
        className={`fixed inset-y-0 right-0 w-[480px] bg-white shadow-xl z-40 flex flex-col transition-transform duration-300 ${
          open ? "translate-x-0" : "translate-x-full"
        }`}
      >
        {/* Header */}
        <div className="h-14 flex-shrink-0 flex items-center justify-between px-5 border-b border-slate-200/80">
          <h2 className="text-sm font-semibold text-slate-900">Task Details</h2>
          <button
            type="button"
            onClick={onClose}
            className="p-1.5 rounded-lg text-slate-400 hover:text-slate-600 hover:bg-slate-100/60 transition-colors"
            aria-label="Close panel"
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

        {/* Body */}
        <div className="flex-1 overflow-y-auto px-5 py-5">
          {/* Agent pill — only when session_id is set. Links to the session. */}
          {task?.session_id && (
            <Link
              to={ROUTES.session(task.session_id)}
              className="flex items-center gap-2 bg-brand-amber-50 border border-brand-amber-200/60 rounded-lg px-3 py-2 text-sm text-brand-amber-700 mb-5 hover:bg-brand-amber-100/60 transition-colors"
            >
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={2.5}
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z" />
              </svg>
              <span className="font-medium">Agent session</span>
              <span className="text-[10px] font-mono text-brand-amber-500 ml-auto">
                {task.session_id.slice(0, 8)}
              </span>
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={2}
                strokeLinecap="round"
                strokeLinejoin="round"
              >
                <polyline points="9 18 15 12 9 6" />
              </svg>
            </Link>
          )}

          <div className="space-y-4">
            <Field label="Title">
              <input
                type="text"
                value={draft.title}
                onChange={(e) => setDraft((d) => ({ ...d, title: e.target.value }))}
                className="h-10 w-full px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
              />
            </Field>

            <Field label="Description">
              <textarea
                rows={3}
                value={draft.description}
                onChange={(e) => setDraft((d) => ({ ...d, description: e.target.value }))}
                className="w-full px-3.5 py-2 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150 resize-none"
                placeholder="What needs to be done?"
              />
            </Field>

            <Field label="Status">
              <select
                value={draft.status}
                onChange={(e) => setDraft((d) => ({ ...d, status: e.target.value as TaskStatus }))}
                className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
              >
                <option value="active">active</option>
                <option value="done">done</option>
                <option value="archived">archived</option>
              </select>
            </Field>

            <Field label="Acceptance Criteria">
              <textarea
                rows={3}
                value={draft.acceptance_criteria}
                onChange={(e) => setDraft((d) => ({ ...d, acceptance_criteria: e.target.value }))}
                className="w-full px-3.5 py-2 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150 resize-none"
                placeholder="How do we know this is done?"
              />
            </Field>

            <Field label="Completion Summary">
              <textarea
                rows={3}
                value={draft.completion_summary}
                onChange={(e) => setDraft((d) => ({ ...d, completion_summary: e.target.value }))}
                className="w-full px-3.5 py-2 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150 resize-none"
                placeholder="What was done…"
              />
            </Field>

            <Field label="Verification Report">
              <textarea
                rows={3}
                value={draft.verification_report}
                onChange={(e) => setDraft((d) => ({ ...d, verification_report: e.target.value }))}
                className="w-full px-3.5 py-2 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150 resize-none"
                placeholder="QA notes, test results…"
              />
            </Field>
          </div>
        </div>

        {/* Footer */}
        <div className="h-16 flex-shrink-0 flex items-center justify-between px-5 border-t border-slate-200/80 bg-white">
          <button
            type="button"
            onClick={() => task && onDelete(task.id)}
            disabled={!task || isDeleting}
            className="h-9 px-3 text-xs font-medium text-brand-red-600 bg-brand-red-50 border border-brand-red-200/60 rounded-lg hover:bg-brand-red-100 transition-colors disabled:opacity-50"
          >
            {isDeleting ? "Deleting…" : "Delete"}
          </button>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onClose}
              className="h-9 px-3 text-sm font-medium text-slate-700 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 transition-colors"
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={handleSave}
              disabled={!canSave}
              className="h-9 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-colors disabled:opacity-50"
            >
              {isSaving ? "Saving…" : "Save"}
            </button>
          </div>
        </div>
      </aside>
    </>
  );
}

const blankDraft = {
  title: "",
  description: "",
  status: "active" as TaskStatus,
  acceptance_criteria: "",
  completion_summary: "",
  verification_report: "",
};

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <label className="block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5">
        {label}
      </label>
      {children}
    </div>
  );
}
