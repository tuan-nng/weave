// NewSessionModal — shared "New Session" form bound to a single workspace.
// Used by:
//   - web/src/app/pages/workspace.tsx (page-header trigger)
//   - web/src/app/pages/sessions.tsx (per-workspace trigger on the list page)
//
// Open/close control: parent owns `useState<string | null>(null)` and passes
// `workspaceId={id-or-null}`. Matches the CreateBoardModal / CreateCodebaseModal
// precedent (boards.tsx, codebases.tsx). Form state, provider/specialist
// fetch, and the useCreateSession mutation are owned by this component.
// Error is rendered inline so the modal is fully self-contained — no
// parent-level banner required. The optional `onCreated` callback fires
// AFTER the modal has closed on a successful create; the caller decides
// what to do (typically: navigate to the new session's detail page).

import { useEffect, useState } from "react";
import { Modal } from "./modal";
import { useCreateSession } from "../hooks/use-workspaces";
import { useProviders } from "../hooks/use-providers";
import { useSpecialists } from "../hooks/use-specialists";
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
  // useCreateSession is bound to a single workspace. We always call the
  // hook (rules of hooks) but pass "" while closed — the mutation is
  // only fired from inside handleSubmit, which re-checks workspaceId,
  // so the empty-string case is unreachable in practice.
  const createSession = useCreateSession(workspaceId ?? "");

  const [providerId, setProviderId] = useState("");
  const [specialistId, setSpecialistId] = useState(""); // "" = no specialist
  const [model, setModel] = useState("");
  const [error, setError] = useState<string | null>(null);

  // Reset form + error on every open transition (mount, or null→id, or
  // id1→id2). Does not fire on close (id→null).
  useEffect(() => {
    if (workspaceId !== null) {
      setProviderId("");
      setSpecialistId("");
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
    <Modal open={workspaceId !== null} onClose={handleClose}>
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
    </Modal>
  );
}
