// AddColumnModal — small form to create a new column. Auto-trigger
// requires a specialist_id (per the backend's `validate_auto_trigger`
// guard at `crates/weave-server/src/store/columns.rs:181-188`), so the
// specialist selector is enabled only when the auto-trigger toggle is
// on. The specialists list comes from the existing
// `api.specialists.list()` endpoint.
//
// feat-068 (F-2 / F-3 / F-5 / F-7): widened the form to expose
// `stage` (Backlog / To Do / In Progress / Review / Done) and
// `runtime_kind` (the registered Runtime Tools) — both of which
// the backend `CreateColumnRequest` already accepts but the
// previous UI did not send. Also shows the specialist's
// `description` as a secondary line under each option in the
// specialist dropdown (F-7).

import { useState } from "react";
import { Modal } from "../../../components/modal";
import { useSpecialists } from "../../../hooks/use-specialists";
import type { ColumnStage, CreateColumnRequest, RuntimeKind } from "../../../lib/types";

interface AddColumnModalProps {
  open: boolean;
  onClose: () => void;
  onSubmit: (data: CreateColumnRequest) => void;
  isSubmitting: boolean;
}

/// Display order for the Stage radio. Matches the canonical pipeline
/// order (Backlog → To Do → … → Done) so the radio's visual order
/// matches the lane position the user will create.
const STAGE_OPTIONS: ReadonlyArray<{ value: ColumnStage; label: string }> = [
  { value: "backlog", label: "Backlog" },
  { value: "todo", label: "To Do" },
  { value: "dev", label: "In Progress" },
  { value: "review", label: "Review" },
  { value: "done", label: "Done" },
];

/// Runtime Tool options exposed in the modal. Mirrors the `RuntimeKind`
/// type in `web/src/lib/types.ts`. The backend's column-level
/// `runtime_kind` binding (feat-055) controls which provider is picked
/// for the auto-spawned session; HTTP kinds pick an HTTP provider, CLI
/// kinds pick a CLI provider. The "Inherit" option is the pre-feat-055
/// behavior: fall back to the provider registry's first healthy
/// provider for the column's specialist.
const RUNTIME_OPTIONS: ReadonlyArray<{ value: RuntimeKind | "inherit"; label: string }> = [
  { value: "inherit", label: "Inherit from provider" },
  { value: "anthropic-api", label: "Anthropic API" },
  { value: "claude-code", label: "Claude Code (CLI)" },
  { value: "codex", label: "Codex (CLI)" },
  { value: "opencode", label: "OpenCode (CLI)" },
];

export function AddColumnModal({ open, onClose, onSubmit, isSubmitting }: AddColumnModalProps) {
  const [name, setName] = useState("");
  const [autoTrigger, setAutoTrigger] = useState(false);
  const [specialistId, setSpecialistId] = useState("");
  // Default to "todo" for new auto-trigger columns (matches the
  // most common user intent) and "dev" for non-auto columns. The
  // backend's "dev" default is preserved if the user never changes
  // the radio; explicit "todo" here closes the "wrong next stage"
  // prompt bug for the common case.
  const [stage, setStage] = useState<ColumnStage>("todo");
  const [runtimeKind, setRuntimeKind] = useState<RuntimeKind | "inherit">("inherit");

  // `useSpecialists` is a thin TanStack Query wrapper around
  // `api.specialists.list()`. Available because the session page
  // already uses it (web/src/hooks/use-specialists.ts).
  const { data: specialists = [] } = useSpecialists();

  function handleClose() {
    if (isSubmitting) return;
    setName("");
    setAutoTrigger(false);
    setSpecialistId("");
    setStage("todo");
    setRuntimeKind("inherit");
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
      stage,
      runtime_kind: runtimeKind === "inherit" ? undefined : runtimeKind,
    });
    setName("");
    setAutoTrigger(false);
    setSpecialistId("");
    setStage("todo");
    setRuntimeKind("inherit");
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

        <div>
          <label className="block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5">
            Stage
          </label>
          <div className="grid grid-cols-5 gap-1.5" role="radiogroup" aria-label="Stage">
            {STAGE_OPTIONS.map((opt) => (
              <button
                key={opt.value}
                type="button"
                role="radio"
                aria-checked={stage === opt.value}
                onClick={() => setStage(opt.value)}
                className={`h-9 px-2 rounded-lg border text-[11px] font-medium transition-colors ${
                  stage === opt.value
                    ? "border-brand-blue-400 bg-brand-blue-50/60 text-slate-900"
                    : "border-slate-200 bg-white text-slate-700 hover:bg-slate-50"
                }`}
              >
                {opt.label}
              </button>
            ))}
          </div>
          <p className="text-[10px] text-slate-400 mt-1.5">
            Drives the orchestrator's "advance to &lt;stage&gt;" instruction. Defaults to "To Do"
            for new auto-trigger columns.
          </p>
        </div>

        <div>
          <label
            htmlFor="add-column-runtime"
            className="block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5"
          >
            Runtime Tool
          </label>
          <select
            id="add-column-runtime"
            value={runtimeKind}
            onChange={(e) => setRuntimeKind(e.target.value as RuntimeKind | "inherit")}
            className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
          >
            {RUNTIME_OPTIONS.map((opt) => (
              <option key={opt.value} value={opt.value}>
                {opt.label}
              </option>
            ))}
          </select>
          <p className="text-[10px] text-slate-400 mt-1.5">
            Pins the auto-spawned session to a specific Runtime Tool. "Inherit" picks the first
            healthy provider (the pre-feat-055 behavior).
          </p>
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
            <label
              htmlFor="add-column-specialist"
              className="block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5"
            >
              Specialist <span className="text-brand-red-500">*</span>
            </label>
            <select
              id="add-column-specialist"
              value={specialistId}
              onChange={(e) => setSpecialistId(e.target.value)}
              className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
            >
              <option value="">Select a specialist…</option>
              {specialists.map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name}
                  {s.description ? ` — ${s.description}` : ""}
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
