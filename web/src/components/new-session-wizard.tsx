// NewSessionWizard — 4-step modal that replaces the legacy single-page
// NewSessionModal. The provider schema was widened in feat-039 to a
// discriminated union (HTTP vs CLI); the runtime × mode matrix was
// added in feat-040; per-runtime model caching landed in feat-042.
// The frontend was the lagging piece: a Runtime Tool picker was
// missing, and the server's `runtime_mode_incompatible` /
// `cwd_outside_codebase` errors had no UX surface.
//
// The 4 steps are:
//   0. Runtime Tool (the chosen `Provider.kind` + its healthy flag
//      filters non-selectable rows out of the next steps)
//   1. Role (specialist, filtered by the chosen runtime's compat matrix)
//   2. Model (per-provider model list, pre-select first entry)
//   3. Workspace+Codebase+Task (existing codebases + unbound tasks)
//
// The wizard owns its form state in a `useReducer` (step transitions
// and error-jumps form a finite state machine), all five data
// queries, and the create-session mutation. The public surface
// (`workspaceId`, `onClose`, `onCreated`) matches the legacy modal
// so the call sites in `pages/workspace.tsx` and `pages/sessions.tsx`
// swap in-place.
//
// Error rendering: inline `role="alert"` at the top. On the
// server-side codes `runtime_mode_incompatible` and
// `cwd_outside_codebase` the wizard jumps to the failing step
// (the user can fix and re-submit). Other codes render inline with
// no jump.

import { useEffect, useMemo, useReducer, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Modal } from "./modal";
import { NewCodebaseModal } from "./new-codebase-modal";
import { useCreateSession } from "../hooks/use-workspaces";
import { useCodebases } from "../hooks/use-codebase";
import { useProviders, useProviderModels } from "../hooks/use-providers";
import { useSpecialists } from "../hooks/use-specialists";
import { useUnboundTasks } from "../hooks/use-tasks";
import { ROUTES } from "../lib/routes";
import {
  defaultRuntimeForProviderKind,
  isProfileCompatible,
  type RuntimeKind,
  type SessionMode,
} from "../lib/runtime-matrix";
import type { CreateSessionRequest, Provider, Session } from "../lib/types";
import { ApiError } from "../lib/api";

const FIELD_CLASS =
  "w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150";
const LABEL_CLASS =
  "block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5";
const STEP_LABELS = ["Runtime", "Role", "Model", "Workspace"] as const;
const TOTAL_STEPS = STEP_LABELS.length;

interface NewSessionWizardProps {
  /** null = closed; non-null = open and bound to this workspace. */
  workspaceId: string | null;
  onClose: () => void;
  /** Fires AFTER the wizard has closed on a successful create. */
  onCreated?: (session: Session) => void;
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

interface WizardState {
  step: 0 | 1 | 2 | 3;
  /// Empty string = none / unselected. The backend DTO accepts
  /// `undefined` for these; we use `""` as the form sentinel so a
  /// `select` with no value can show the placeholder.
  providerId: string;
  specialistId: string;
  model: string;
  codebaseId: string;
  taskId: string;
  /// Wizard-level error to surface at the top of the panel. The
  /// optional `step` is the step we want the user to land on when
  /// they dismiss the error and try again — populated by the two
  /// server error codes that imply a specific step's choice is wrong.
  submitError: { code: string; message: string; jumpStep?: 0 | 1 | 2 | 3 } | null;
  /// Tracks which provider the current `model` value was preloaded
  /// for. If `null`, the model is either empty (user cleared it) or
  /// untouched. If it equals `providerId`, the value is the
  /// preloaded default and the next provider change should clear
  /// the field. If it doesn't match `providerId`, the user has
  /// typed something custom and we should not overwrite it.
  preloadedForProvider: string | null;
}

type WizardAction =
  | {
      kind: "set";
      field: "providerId" | "specialistId" | "model" | "codebaseId" | "taskId";
      value: string;
    }
  | { kind: "preloadModel"; value: string; providerId: string }
  | { kind: "resetPreload" }
  | { kind: "goto"; step: 0 | 1 | 2 | 3 }
  | { kind: "back" }
  | { kind: "next" }
  | { kind: "error"; error: WizardState["submitError"] }
  | { kind: "reset" };

const INITIAL_STATE: WizardState = {
  step: 0,
  providerId: "",
  specialistId: "",
  model: "",
  codebaseId: "",
  taskId: "",
  submitError: null,
  preloadedForProvider: null,
};

function wizardReducer(state: WizardState, action: WizardAction): WizardState {
  switch (action.kind) {
    case "set":
      return { ...state, [action.field]: action.value };
    case "preloadModel":
      return { ...state, model: action.value, preloadedForProvider: action.providerId };
    case "resetPreload":
      // Fired when the chosen provider changes — clears the
      // `preloadedForProvider` flag so the next model-list arrival
      // pre-selects the new provider's first model. The model
      // value itself is also cleared so the input is blank on
      // the next render.
      return { ...state, model: "", preloadedForProvider: null };
    case "goto":
      return { ...state, step: action.step, submitError: null };
    case "back":
      return state.step === 0
        ? state
        : { ...state, step: (state.step - 1) as WizardState["step"], submitError: null };
    case "next":
      return state.step === TOTAL_STEPS - 1
        ? state
        : { ...state, step: (state.step + 1) as WizardState["step"], submitError: null };
    case "error":
      if (action.error === null) {
        return { ...state, submitError: null };
      }
      return {
        ...state,
        submitError: action.error,
        ...(action.error.jumpStep !== undefined ? { step: action.error.jumpStep } : {}),
      };
    case "reset":
      return INITIAL_STATE;
  }
}

/// Map a server `ApiError.code` to the step the user should land on.
/// Only the two codes that imply a *specific* step's choice is wrong
/// get a jump; other codes render inline at the current step.
function jumpStepForErrorCode(code: string): 0 | 1 | 2 | 3 | undefined {
  switch (code) {
    case "runtime_mode_incompatible":
      return 0; // Runtime tool
    case "cwd_outside_codebase":
      return 3; // Workspace + Codebase + Task
    default:
      return undefined;
  }
}

// ---------------------------------------------------------------------------
// Custom hook — owns the reducer, the 5 data queries, and the mutation
// ---------------------------------------------------------------------------

interface UseNewSessionWizard {
  state: WizardState;
  dispatch: React.Dispatch<WizardAction>;
  providers: Provider[] | undefined;
  providersLoading: boolean;
  /// Only the providers whose 10s `HealthCache` is currently `true`.
  /// Cold-cache providers (never probed) are `healthy: false` and
  /// are excluded from this list. The Step 0 picker renders them as
  /// greyed-out non-selectable rows.
  healthyProviders: Provider[];
  selectedProvider: Provider | undefined;
  /// Specialists filtered by the chosen runtime's compat matrix.
  compatibleSpecialists: ReturnType<typeof useSpecialists>["data"];
  codebases: ReturnType<typeof useCodebases>["data"];
  unboundTasks: ReturnType<typeof useUnboundTasks>["data"];
  resolvedRuntimeKind: RuntimeKind | null;
  resolvedMode: SessionMode | null;
  /// Per-provider model list. Lazily fetched on Step 2.
  providerModels: ReturnType<typeof useProviderModels>;
  createSession: ReturnType<typeof useCreateSession>;
  /// True iff the user may click "Next" on the current step. The
  /// caller passes this to the Next button's `disabled` prop.
  canAdvance: boolean;
  canSubmit: boolean;
}

function useNewSessionWizard(workspaceId: string | null): UseNewSessionWizard {
  const [state, dispatch] = useReducer(wizardReducer, INITIAL_STATE);
  const queryClient = useQueryClient();

  const providersQuery = useProviders();
  const specialistsQuery = useSpecialists();
  const codebasesQuery = useCodebases(workspaceId ?? "");
  const tasksQuery = useUnboundTasks(workspaceId);
  const createSession = useCreateSession(workspaceId ?? "");

  const selectedProvider = useMemo(
    () => providersQuery.data?.find((p) => p.id === state.providerId),
    [providersQuery.data, state.providerId],
  );

  // Resolve the runtime kind/mode eagerly. Step 0 only writes the
  // provider id; the resolved pair comes from
  // `defaultRuntimeForProviderKind` (or, in the future, an explicit
  // runtime picker on Step 0). The wizard sends both fields on submit
  // so the server never has to fall back to defaults.
  const resolvedRuntime = useMemo(() => {
    if (!selectedProvider) return null;
    return defaultRuntimeForProviderKind(selectedProvider.kind);
  }, [selectedProvider]);

  const compatibleSpecialists = useMemo(() => {
    const all = specialistsQuery.data ?? [];
    if (resolvedRuntime === null) return all;
    return all.filter((s) => isProfileCompatible(resolvedRuntime.runtimeKind, s.tool_profile));
  }, [specialistsQuery.data, resolvedRuntime]);

  const healthyProviders = useMemo(
    () => (providersQuery.data ?? []).filter((p) => p.healthy),
    [providersQuery.data],
  );

  // Reset the form on every workspace transition. The session flow
  // binds to one workspace at a time; switching workspaces (e.g. by
  // closing and re-opening on a different id) is a clean slate.
  useEffect(() => {
    dispatch({ kind: "reset" });
  }, [workspaceId]);

  // Provider change → reset the model + the preloaded-for marker
  // AND wipe the cached model list for any previous provider. The
  // wipe is what makes the pre-select effect race-safe: without it,
  // the effect can fire on a render where the new provider is
  // selected but `modelsQuery.data` is still the OLD provider's
  // list, and we'd pre-select a model id that doesn't belong to the
  // new provider. Removing the old cache forces `modelsQuery.data`
  // to be `undefined` until the new fetch resolves.
  //
  // The query key for `useProviderModels` is
  // `["api", "providers", "models", providerId]`, so the prefix
  // `["api", "providers", "models"]` matches all variants.
  useEffect(() => {
    if (state.providerId === "") return;
    dispatch({ kind: "resetPreload" });
    queryClient.removeQueries({ queryKey: ["api", "providers", "models"], exact: false });
  }, [state.providerId, queryClient]);

  const modelsQuery = useProviderModels(selectedProvider?.id ?? null);

  // Pre-select the first model on provider change. We guard on
  // BOTH `preloadedForProvider === providerId` AND
  // `state.providerId === selectedProvider.id` (the latter guards
  // against a race where `modelsQuery.data` briefly carries the
  // OLD provider's list during a key transition, even after the
  // cache-wipe effect above). The user can still type a custom
  // value; the marker is only set when the wizard itself writes
  // the field, so a manual `set model "..."` does NOT touch the
  // marker and a subsequent refetch won't overwrite.
  useEffect(() => {
    if (selectedProvider === undefined) return;
    if (state.preloadedForProvider === selectedProvider.id) return;
    // The cache-wipe effect races the data swap: the new provider
    // may be selected before the new models list arrives, leaving
    // `modelsQuery.data` briefly bound to the previous provider.
    // The `state.providerId === selectedProvider.id` guard short-
    // circuits the effect in that window — `state.providerId` is
    // the source of truth and updates synchronously via dispatch.
    if (state.providerId !== selectedProvider.id) return;
    const first = modelsQuery.data?.[0];
    if (first === undefined) return;
    dispatch({
      kind: "preloadModel",
      value: first.id,
      providerId: selectedProvider.id,
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedProvider, modelsQuery.data, state.providerId]);

  const canAdvance = useMemo(() => {
    if (state.step === 0) {
      // Step 0: a healthy provider must be selected.
      return state.providerId !== "" && Boolean(selectedProvider?.healthy);
    }
    if (state.step === 1) {
      // Step 1: specialist is optional, always advanceable.
      return true;
    }
    if (state.step === 2) {
      // Step 2: model is optional (empty = provider default).
      return true;
    }
    // Step 3: codebase + task are both optional.
    return true;
  }, [state.step, state.providerId, selectedProvider]);

  return {
    state,
    dispatch,
    providers: providersQuery.data,
    providersLoading: providersQuery.isLoading,
    healthyProviders,
    selectedProvider,
    compatibleSpecialists,
    codebases: codebasesQuery.data,
    unboundTasks: tasksQuery.data,
    resolvedRuntimeKind: resolvedRuntime?.runtimeKind ?? null,
    resolvedMode: resolvedRuntime?.mode ?? null,
    providerModels: modelsQuery,
    createSession,
    canAdvance,
    // The Submit button stays disabled unless the chosen provider
    // is still healthy at submit time. Without the health check,
    // a user who picks a healthy provider, advances to Step 3, then
    // goes back to Step 0 and switches to an unhealthy provider
    // could submit from Step 3 with an unselectable row and the
    // server would 400. Gating on `healthy` at submit time mirrors
    // the Step 0 Next-button logic.
    canSubmit: state.providerId !== "" && Boolean(selectedProvider?.healthy),
  };
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export function NewSessionWizard({ workspaceId, onClose, onCreated }: NewSessionWizardProps) {
  const open = workspaceId !== null;
  const w = useNewSessionWizard(workspaceId);

  // Nested NewCodebaseModal — the user can register a codebase
  // without leaving the wizard. `zIndex={60}` stacks it above the
  // wizard's z-50 backdrop; the outer's `closeOnEscape={!showNew}`
  // keeps Escape bound to the inner modal first.
  const [showNewCodebase, setShowNewCodebase] = useState(false);

  function handleClose() {
    if (w.createSession.isPending) return;
    onClose();
  }

  function handleSubmit() {
    if (workspaceId === null) return;
    if (w.state.providerId === "") return;
    const payload: CreateSessionRequest = {
      provider_id: w.state.providerId,
      specialist_id: w.state.specialistId === "" ? undefined : w.state.specialistId,
      model: w.state.model.trim() === "" ? undefined : w.state.model.trim(),
      codebase_id: w.state.codebaseId === "" ? undefined : w.state.codebaseId,
      // The resolved pair is always non-null on the submit path
      // (canSubmit requires a provider), so the non-null assertions
      // here are safe.
      runtime_kind: w.resolvedRuntimeKind ?? undefined,
      mode: w.resolvedMode ?? undefined,
    };
    w.createSession.mutate(payload, {
      onSuccess: (session) => {
        onClose();
        onCreated?.(session);
      },
      onError: (err) => {
        // Distinguish `ApiError` (we know its shape) from a generic
        // thrown `Error` (network failure, etc.). The two take
        // different code paths so the user-facing banner is
        // accurate in both cases.
        const code = err instanceof ApiError ? err.code : "unknown";
        const message = err instanceof Error ? err.message : "Failed to create session";
        w.dispatch({
          kind: "error",
          error: { code, message, jumpStep: jumpStepForErrorCode(code) },
        });
      },
    });
  }

  return (
    <Modal open={open} onClose={handleClose} closeOnEscape={!showNewCodebase}>
      <div className="flex flex-col gap-5">
        <Header
          step={w.state.step}
          onBack={() => w.dispatch({ kind: "back" })}
          onClose={handleClose}
          disabled={w.createSession.isPending}
        />

        {w.state.submitError !== null && (
          <div
            role="alert"
            className="rounded-lg border border-brand-red-200/60 bg-brand-red-50 px-3 py-2 text-xs text-brand-red-700"
          >
            {w.state.submitError.message}
          </div>
        )}

        {w.state.step === 0 && (
          <StepProvider
            providers={w.providers}
            healthyProviders={w.healthyProviders}
            selectedId={w.state.providerId}
            onChange={(id) => w.dispatch({ kind: "set", field: "providerId", value: id })}
          />
        )}

        {w.state.step === 1 && (
          <StepSpecialist
            specialists={w.compatibleSpecialists ?? []}
            selectedId={w.state.specialistId}
            onChange={(id) => w.dispatch({ kind: "set", field: "specialistId", value: id })}
          />
        )}

        {w.state.step === 2 && (
          <StepModel
            models={w.providerModels.data}
            isLoading={w.providerModels.isLoading}
            model={w.state.model}
            onChange={(v) => w.dispatch({ kind: "set", field: "model", value: v })}
          />
        )}

        {w.state.step === 3 && (
          <StepWorkspace
            codebases={w.codebases ?? []}
            tasks={w.unboundTasks ?? []}
            codebaseId={w.state.codebaseId}
            taskId={w.state.taskId}
            onCodebaseChange={(id) => w.dispatch({ kind: "set", field: "codebaseId", value: id })}
            onTaskChange={(id) => w.dispatch({ kind: "set", field: "taskId", value: id })}
            onRegisterCodebase={() => setShowNewCodebase(true)}
          />
        )}

        <Footer
          step={w.state.step}
          isPending={w.createSession.isPending}
          canAdvance={w.canAdvance}
          canSubmit={w.canSubmit}
          onBack={() => w.dispatch({ kind: "back" })}
          onNext={() => w.dispatch({ kind: "next" })}
          onSubmit={handleSubmit}
        />
      </div>

      {workspaceId !== null && (
        <NewCodebaseModal
          workspaceId={showNewCodebase ? workspaceId : null}
          onClose={() => setShowNewCodebase(false)}
          zIndex={60}
          onCreated={(cb) => w.dispatch({ kind: "set", field: "codebaseId", value: cb.id })}
        />
      )}
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// Step components
// ---------------------------------------------------------------------------

function Header({
  step,
  onBack,
  onClose,
  disabled,
}: {
  step: 0 | 1 | 2 | 3;
  onBack: () => void;
  onClose: () => void;
  disabled: boolean;
}) {
  return (
    <div className="flex items-center justify-between">
      <div className="flex items-center gap-3">
        <button
          type="button"
          onClick={onBack}
          disabled={step === 0 || disabled}
          aria-label="Previous step"
          className="text-slate-400 hover:text-slate-600 transition-colors disabled:opacity-30 disabled:cursor-not-allowed"
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
            <polyline points="15 18 9 12 15 6" />
          </svg>
        </button>
        <div>
          <h3 className="text-lg font-semibold text-slate-900">New Session</h3>
          <p className="text-[11px] text-slate-500">
            Step {step + 1} of {TOTAL_STEPS} · {STEP_LABELS[step]}
          </p>
        </div>
      </div>
      <button
        type="button"
        onClick={onClose}
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
  );
}

function StepProvider({
  providers,
  healthyProviders,
  selectedId,
  onChange,
}: {
  providers: Provider[] | undefined;
  healthyProviders: Provider[];
  selectedId: string;
  onChange: (id: string) => void;
}) {
  // The healthy set may be empty even if `providers` has rows. We
  // show the empty state with a "configure a provider" link in that
  // case so the user has a path forward.
  if (providers && providers.length > 0 && healthyProviders.length === 0) {
    return (
      <div>
        <label className={LABEL_CLASS}>Runtime Tool</label>
        <p className="text-sm text-slate-600">
          No healthy providers. Visit{" "}
          <a href={ROUTES.settings} className="text-brand-blue-600 hover:underline">
            Settings
          </a>{" "}
          to configure a provider.
        </p>
      </div>
    );
  }
  const options = providers ?? [];
  return (
    <div>
      <label className={LABEL_CLASS}>
        Runtime Tool <span className="text-brand-red-500">*</span>
      </label>
      <select
        className={FIELD_CLASS}
        value={selectedId}
        onChange={(e) => onChange(e.target.value)}
        autoFocus
      >
        <option value="">Select provider…</option>
        {options.map((p) => (
          <option key={p.id} value={p.id} disabled={!p.healthy}>
            {p.name}
            {p.healthy ? "" : " (unhealthy)"}
          </option>
        ))}
      </select>
    </div>
  );
}

function StepSpecialist({
  specialists,
  selectedId,
  onChange,
}: {
  specialists: { name: string; tool_profile: string | null }[];
  selectedId: string;
  onChange: (id: string) => void;
}) {
  return (
    <div>
      <label className={LABEL_CLASS}>Role</label>
      <select
        className={FIELD_CLASS}
        value={selectedId}
        onChange={(e) => onChange(e.target.value)}
        autoFocus
      >
        <option value="">No specialist</option>
        {specialists.map((s) => (
          <option key={s.name} value={s.name}>
            {s.name}
          </option>
        ))}
      </select>
      <p className="mt-1.5 text-[11px] text-slate-500">
        Filtered to specialists compatible with the chosen runtime.
      </p>
    </div>
  );
}

function StepModel({
  models,
  isLoading,
  model,
  onChange,
}: {
  models: { id: string; name: string }[] | undefined;
  isLoading: boolean;
  model: string;
  onChange: (v: string) => void;
}) {
  if (isLoading) {
    return (
      <div>
        <label className={LABEL_CLASS}>Model</label>
        <p className="text-sm text-slate-500">Loading models…</p>
      </div>
    );
  }
  return (
    <div>
      <label className={LABEL_CLASS}>Model</label>
      <input
        className={FIELD_CLASS}
        type="text"
        value={model}
        onChange={(e) => onChange(e.target.value)}
        placeholder="Leave empty for provider default"
        autoFocus
      />
      {models && models.length > 0 && (
        <p className="mt-1.5 text-[11px] text-slate-500">
          Available: {models.map((m) => m.name).join(", ")}
        </p>
      )}
    </div>
  );
}

function StepWorkspace({
  codebases,
  tasks,
  codebaseId,
  taskId,
  onCodebaseChange,
  onTaskChange,
  onRegisterCodebase,
}: {
  codebases: { id: string; path: string; label: string | null }[];
  tasks: { id: string; title: string }[];
  codebaseId: string;
  taskId: string;
  onCodebaseChange: (id: string) => void;
  onTaskChange: (id: string) => void;
  onRegisterCodebase: () => void;
}) {
  return (
    <div className="space-y-4">
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
                onClick={onRegisterCodebase}
                className="text-brand-blue-600 hover:underline"
              >
                Register a codebase
              </button>{" "}
              first, or continue without one.
            </p>
          </>
        ) : (
          <select
            className={FIELD_CLASS}
            value={codebaseId}
            onChange={(e) => onCodebaseChange(e.target.value)}
            autoFocus
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
      <div>
        <label className={LABEL_CLASS}>Task (optional)</label>
        <select
          className={FIELD_CLASS}
          value={taskId}
          onChange={(e) => onTaskChange(e.target.value)}
        >
          <option value="">No task</option>
          {tasks.map((t) => (
            <option key={t.id} value={t.id}>
              {t.title}
            </option>
          ))}
        </select>
        <p className="mt-1.5 text-[11px] text-slate-500">
          Bind this session to an active backlog card.
        </p>
      </div>
    </div>
  );
}

function Footer({
  step,
  isPending,
  canAdvance,
  canSubmit,
  onBack,
  onNext,
  onSubmit,
}: {
  step: 0 | 1 | 2 | 3;
  isPending: boolean;
  canAdvance: boolean;
  canSubmit: boolean;
  onBack: () => void;
  onNext: () => void;
  onSubmit: () => void;
}) {
  const isLast = step === TOTAL_STEPS - 1;
  return (
    <div className="flex justify-end gap-3 pt-2">
      <button
        type="button"
        onClick={onBack}
        disabled={step === 0 || isPending}
        className="h-10 px-4 text-sm font-medium text-slate-700 bg-white border border-slate-200 rounded-xl hover:bg-slate-50 transition-all duration-150 disabled:opacity-50"
      >
        Back
      </button>
      {isLast ? (
        <button
          type="button"
          onClick={onSubmit}
          disabled={isPending || !canSubmit}
          className="h-10 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-all duration-150 disabled:opacity-50"
        >
          {isPending ? "Creating…" : "Create Session"}
        </button>
      ) : (
        <button
          type="button"
          onClick={onNext}
          disabled={!canAdvance}
          className="h-10 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-all duration-150 disabled:opacity-50"
        >
          Next
        </button>
      )}
    </div>
  );
}
