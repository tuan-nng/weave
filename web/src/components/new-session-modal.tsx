// NewSessionModal — shared "New Session" form bound to a single workspace.
// Used by:
//   - web/src/app/pages/workspace.tsx (page-header trigger)
//   - web/src/app/pages/sessions.tsx (per-workspace trigger on the list page)
//
// Open/close control: parent owns `useState<string | null>(null)` and passes
// `workspaceId={id-or-null}`. Matches the CreateBoardModal / CreateCodebaseModal
// precedent (boards.tsx, codebases.tsx). Form state, provider/specialist/codebase
// fetch, and the useCreateSession mutation are owned by this component.
// Error is rendered inline so the modal is fully self-contained — no
// parent-level banner required. The optional `onCreated` callback fires
// AFTER the modal has closed on a successful create; the caller decides
// what to do (typically: navigate to the new session's detail page).

import { useEffect, useState } from "react";
import { Modal } from "./modal";
import { NewCodebaseModal } from "./new-codebase-modal";
import { useCreateSession } from "../hooks/use-workspaces";
import { useProviders } from "../hooks/use-providers";
import { useSpecialists } from "../hooks/use-specialists";
import { useCodebases } from "../hooks/use-codebase";
import type { CreateSessionRequest, Session } from "../lib/types";

interface NewSessionModalProps {
  /** null = closed; non-null = open and bound to this workspace. */
  workspaceId: string | null;
  onClose: () => void;
  /** Fires AFTER the modal has closed on a successful create. */
  onCreated?: (session: Session) => void;
}

const FIELD_CLASS =
  "w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150";
const LABEL_CLASS =
  "block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5";

export function NewSessionModal({ workspaceId, onClose, onCreated }: NewSessionModalProps) {
  const { data: providers } = useProviders();
  const { data: specialists = [] } = useSpecialists();
  // Codebase list is workspace-scoped. The hook is bound to the
  // modal's workspace and only fires when the modal is open (i.e.
  // workspaceId is non-null). The "" fallback keeps the rules of
  // hooks happy while the modal is closed — the query is disabled
  // in that case (see use-codebase.ts).
  //
  // `api.codebases.list` returns Codebase[] directly (the apiFetch
  // helper unwraps the {data: T} envelope), so the hook's `data`
  // is the array — there is no nested `.data` to reach into.
  const { data: codebasesResp } = useCodebases(workspaceId ?? "");
  const codebases = codebasesResp ?? [];
  // useCreateSession is bound to a single workspace. We always call the
  // hook (rules of hooks) but pass "" while closed — the mutation is
  // only fired from inside handleSubmit, which re-checks workspaceId,
  // so the empty-string case is unreachable in practice.
  const createSession = useCreateSession(workspaceId ?? "");

  const [providerId, setProviderId] = useState("");
  const [specialistId, setSpecialistId] = useState(""); // "" = no specialist
  const [codebaseId, setCodebaseId] = useState(""); // "" = no codebase
  const [model, setModel] = useState("");
  const [error, setError] = useState<string | null>(null);
  // Inline-create affordance: when a workspace has zero codebases, the
  // "Register a codebase" link is replaced by a button that opens the
  // NewCodebaseModal as a nested modal — the user can register a
  // codebase without leaving the New Session flow. On success the new
  // codebase is auto-selected in the dropdown.
  const [showNewCodebase, setShowNewCodebase] = useState(false);

  // Reset form + error on every open transition (mount, or null→id, or
  // id1→id2). Does not fire on close (id→null).
  useEffect(() => {
    if (workspaceId !== null) {
      setProviderId("");
      setSpecialistId("");
      setCodebaseId("");
      setModel("");
      setError(null);
    }
  }, [workspaceId]);

  function handleClose() {
    if (createSession.isPending) return;
    onClose();
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!workspaceId) return;
    if (!providerId) {
      setError("Provider is required");
      return;
    }
    const trimmedModel = model.trim();
    const payload: CreateSessionRequest = {
      provider_id: providerId,
      specialist_id: specialistId === "" ? undefined : specialistId,
      model: trimmedModel === "" ? undefined : trimmedModel,
      codebase_id: codebaseId === "" ? undefined : codebaseId,
    };
    createSession.mutate(payload, {
      onSuccess: (session) => {
        onClose();
        onCreated?.(session);
      },
      onError: (err) => {
        setError(err instanceof Error ? err.message : "Failed to create session");
      },
    });
  }

  return (
    <Modal open={workspaceId !== null} onClose={handleClose} closeOnEscape={!showNewCodebase}>
      <form onSubmit={handleSubmit} className="space-y-4">
        <div className="flex items-center justify-between mb-6">
          <h3 className="text-lg font-semibold text-slate-900">New Session</h3>
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

        {/* Provider */}
        <div>
          <label className={LABEL_CLASS}>
            Provider <span className="text-brand-red-500">*</span>
          </label>
          <select
            value={providerId}
            onChange={(e) => setProviderId(e.target.value)}
            className={FIELD_CLASS}
            required
            autoFocus
          >
            <option value="">Select provider…</option>
            {providers?.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </div>

        {/* Specialist */}
        <div>
          <label className={LABEL_CLASS}>Specialist</label>
          <select
            value={specialistId}
            onChange={(e) => setSpecialistId(e.target.value)}
            className={FIELD_CLASS}
          >
            <option value="">No specialist</option>
            {specialists.map((s) => (
              <option key={s.name} value={s.name}>
                {s.name}
              </option>
            ))}
          </select>
        </div>

        {/* Codebase */}
        <div>
          <label className={LABEL_CLASS}>Codebase</label>
          {codebases.length === 0 ? (
            <>
              <select className={FIELD_CLASS} disabled value="">
                <option value="">No codebases registered</option>
              </select>
              <p className="mt-1.5 text-[11px] text-slate-500">
                No codebases in this workspace.{" "}
                <button
                  type="button"
                  onClick={() => setShowNewCodebase(true)}
                  className="text-brand-blue-600 hover:underline"
                >
                  Register a codebase
                </button>{" "}
                first, or continue without one.
              </p>
            </>
          ) : (
            <select
              value={codebaseId}
              onChange={(e) => setCodebaseId(e.target.value)}
              className={FIELD_CLASS}
            >
              <option value="">No codebase (operate in workspace root)</option>
              {codebases.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.label ?? c.path}
                </option>
              ))}
            </select>
          )}
        </div>

        {/* Model */}
        <div>
          <label className={LABEL_CLASS}>Model</label>
          <input
            type="text"
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="Leave empty for provider default"
            className={FIELD_CLASS}
          />
        </div>

        {/* Actions */}
        <div className="flex justify-end gap-3 pt-2">
          <button
            type="button"
            onClick={handleClose}
            disabled={createSession.isPending}
            className="h-10 px-4 text-sm font-medium text-slate-700 bg-white border border-slate-200 rounded-xl hover:bg-slate-50 transition-all duration-150"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={createSession.isPending || providerId === ""}
            className="h-10 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-all duration-150 disabled:opacity-50"
          >
            {createSession.isPending ? "Creating…" : "Create Session"}
          </button>
        </div>
      </form>

      {/* Nested NewCodebaseModal: opens on top of this modal so the user
          can register a codebase without leaving the New Session flow.
          `zIndex={60}` stacks it above this modal's z-50 backdrop; the
          outer's `closeOnEscape={!showNewCodebase}` ensures Escape only
          closes the inner modal first. The NewCodebaseModal's own
          useCreateCodebase invalidation causes useCodebases (above) to
          refetch, populating the dropdown with the new entry; the
          onCreated callback then auto-selects it. */}
      {workspaceId !== null && (
        <NewCodebaseModal
          workspaceId={showNewCodebase ? workspaceId : null}
          onClose={() => setShowNewCodebase(false)}
          zIndex={60}
          onCreated={(cb) => setCodebaseId(cb.id)}
        />
      )}
    </Modal>
  );
}
