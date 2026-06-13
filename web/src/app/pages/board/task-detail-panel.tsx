// TaskDetailPanel — slide-over editor for a single task. Reads the
// task from props (the parent passes the canonical `Task` from
// useBoard, so SSE patches propagate without an extra useQuery).
// Saves via the parent's updateTask callback.
//
// feat-068 (F-4 / F-8 / F-10): adds a "Lane" footer showing the
// column's specialist / runtime / stage (F-10), a "Codebase"
// dropdown (F-4) — the user picks a codebase for sessions
// spawned from this card by lane automation; `null` falls
// back to the workspace's first registered codebase, and a
// "Move to column…" button (F-8) that opens a popover with
// the list of columns from the same board. The popover is
// an inline `<details>` element (no portal / no library) —
// the keyboard / focus story is simple, and the popover
// stays inside the slide-over's z-stack.

import { useEffect, useState } from "react";
import { Link } from "react-router";
import { useCodebases } from "../../../hooks/use-codebase";
import { ROUTES } from "../../../lib/routes";
import type { Codebase, Column, Task, TaskStatus, UpdateTaskRequest } from "../../../lib/types";

interface TaskDetailPanelProps {
  task: Task | null;
  /// The full column list (so F-8 "Move to…" can render a picker
  /// and F-10 "Lane" footer can show the current column's
  /// specialist / runtime / stage). Parent passes the columns
  /// from `useBoard().columns` so they stay in sync with the
  /// SSE-patched cache.
  columns: Column[];
  /// The workspace id, used to fetch the codebases list for
  /// the F-4 codebase dropdown.
  workspaceId: string;
  onClose: () => void;
  onSave: (taskId: string, data: UpdateTaskRequest) => void;
  onDelete: (taskId: string) => void;
  /// Move the task to a different column. Parent wires this to
  /// `useBoard().moveTask` (which routes through
  /// `PATCH /api/tasks/:tid` with `column_id` + `position`).
  onMoveToColumn: (taskId: string, toColumnId: string) => void;
  isSaving: boolean;
  isDeleting: boolean;
}

export function TaskDetailPanel({
  task,
  columns,
  workspaceId,
  onClose,
  onSave,
  onDelete,
  onMoveToColumn,
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
    /// Card-level codebase binding (feat-068 F-4). `""` =
    /// "no binding" (server clears the column). When the user
    /// saves without touching the dropdown, we OMIT the
    /// field so the server's tri-state preserves the prior
    /// value.
    codebase_id: string;
  }>(blankDraft);

  const { data: codebases = [] } = useCodebases(workspaceId);
  // F-8: open state for the "Move to…" popover. The popover's
  // button list is only mounted when open so the DOM stays
  // clean (and the in-DOM `<button>` entries don't shadow other
  // matches in tests). Lifting into React state also makes
  // close-on-outside-click trivial — the `<summary>` toggle
  // syncs to this on each click via the `onToggle` handler
  // below.
  const [movePopoverOpen, setMovePopoverOpen] = useState(false);

  useEffect(() => {
    if (task) {
      setDraft({
        title: task.title,
        description: task.description ?? "",
        status: task.status as TaskStatus,
        acceptance_criteria: task.acceptance_criteria ?? "",
        completion_summary: task.completion_summary ?? "",
        verification_report: task.verification_report ?? "",
        codebase_id: task.codebase_id ?? "",
      });
    } else {
      setDraft(blankDraft);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task?.id]);

  const open = task !== null;
  // `hasChanges` — compare to the current canonical task. Disabled
  // Save when no edits OR title is empty.
  const taskCodebaseId = task?.codebase_id ?? "";
  const hasChanges =
    !!task &&
    (draft.title !== task.title ||
      draft.description !== (task.description ?? "") ||
      draft.status !== task.status ||
      draft.acceptance_criteria !== (task.acceptance_criteria ?? "") ||
      draft.completion_summary !== (task.completion_summary ?? "") ||
      draft.verification_report !== (task.verification_report ?? "") ||
      (draft.codebase_id || null) !== (taskCodebaseId || null));
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
    if ((draft.codebase_id || null) !== (taskCodebaseId || null)) {
      data.codebase_id = draft.codebase_id === "" ? null : draft.codebase_id;
    }
    onSave(task.id, data);
  }

  // F-10: Lane footer. Look up the current column by id and render
  // a one-line summary of the binding state.
  const currentColumn = task ? columns.find((c) => c.id === task.column_id) : undefined;

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

          {/* Lane footer (F-10). Below the agent pill, before the
              form fields. Renders the column's binding state so
              the user knows which specialist / runtime / stage
              will pick up the card on the next move. */}
          {currentColumn && (
            <div className="mb-5 text-[11px] text-slate-500 bg-slate-50 border border-slate-200/60 rounded-lg px-3 py-2">
              <span className="font-medium text-slate-700">Lane:</span> {currentColumn.name}
              {currentColumn.specialist_id ? ` (${currentColumn.specialist_id}` : " ("}
              {currentColumn.runtime_kind ? `, ${currentColumn.runtime_kind}` : ""}
              {`,\u00a0stage: ${currentColumn.stage})`}
            </div>
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

            {/* F-4: codebase binding dropdown. The user can pin
                a specific codebase to this card; sessions spawned
                by lane automation will use this codebase's cwd.
                "Workspace default (first codebase)" means the
                binding is empty — the server picks the first
                registered codebase in the workspace. */}
            <Field label="Codebase">
              <select
                value={draft.codebase_id}
                onChange={(e) => setDraft((d) => ({ ...d, codebase_id: e.target.value }))}
                className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
              >
                <option value="">Workspace default (first codebase)</option>
                {codebases.map((cb: Codebase) => (
                  <option key={cb.id} value={cb.id}>
                    {cb.label ?? cb.path}
                    {cb.label ? ` — ${cb.path}` : ""}
                  </option>
                ))}
              </select>
              <p className="text-[10px] text-slate-400 mt-1.5">
                Pinned codebase for sessions spawned by lane automation. "Workspace default" picks
                the first registered codebase when the runtime requires a cwd.
              </p>
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
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => task && onDelete(task.id)}
              disabled={!task || isDeleting}
              className="h-9 px-3 text-xs font-medium text-brand-red-600 bg-brand-red-50 border border-brand-red-200/60 rounded-lg hover:bg-brand-red-100 transition-colors disabled:opacity-50"
            >
              {isDeleting ? "Deleting…" : "Delete"}
            </button>
            {/* F-8: Move to column… (keyboard-accessible fallback
                for the drag-only move). `<details>` keeps the
                focus / escape-key semantics simple. The popover
                button list is only mounted when `open` is true
                so the DOM stays clean and the in-DOM `<button>`
                entries don't shadow other text matches in
                tests / screen readers. */}
            <details
              className="relative"
              data-testid="move-to-popover"
              open={movePopoverOpen}
              onToggle={(e) => setMovePopoverOpen((e.target as HTMLDetailsElement).open)}
            >
              <summary
                className="h-9 px-3 text-xs font-medium text-slate-700 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 transition-colors cursor-pointer list-none inline-flex items-center gap-1 select-none"
                aria-label="Move to column"
              >
                Move to…
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
                  <polyline points="6 9 12 15 18 9" />
                </svg>
              </summary>
              {movePopoverOpen && (
                <div className="absolute bottom-full left-0 mb-1 w-48 max-h-60 overflow-y-auto bg-white border border-slate-200 rounded-lg shadow-lg py-1 z-50">
                  {columns
                    .filter((c) => c.id !== task?.column_id)
                    .map((c) => (
                      <button
                        key={c.id}
                        type="button"
                        onClick={() => {
                          if (!task) return;
                          onMoveToColumn(task.id, c.id);
                          setMovePopoverOpen(false);
                          // Close the panel after the move so the
                          // user can see the new column.
                          onClose();
                        }}
                        className="block w-full text-left px-3 py-1.5 text-xs text-slate-700 hover:bg-slate-50 transition-colors"
                      >
                        {c.name}
                      </button>
                    ))}
                </div>
              )}
            </details>
          </div>
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
  codebase_id: "",
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
