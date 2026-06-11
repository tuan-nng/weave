// AddProviderModal — kind-aware "Add Provider" form (feat-052).
//
// Open/close control: parent owns `useState<boolean>` and passes
// `open={true|false}`. Form state, the kind picker, and the
// `useCreateProvider` mutation are owned by this component. Error is
// rendered inline so the modal is fully self-contained.
//
// Shape: kind picker first (HTTP or CLI), then variant fields. `name`
// and `default_model` are preserved across kind switches; variant
// fields reset to per-kind defaults. Backend `POST /api/providers`
// already accepts the full discriminated shape after feat-039.

import { useEffect, useState } from "react";
import { Modal } from "./modal";
import { useCreateProvider } from "../hooks/use-providers";
import type { CreateProviderRequest } from "../lib/types";

interface AddProviderModalProps {
  open: boolean;
  onClose: () => void;
}

const FIELD_CLASS =
  "w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150";
const LABEL_CLASS =
  "block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5";
const SUBTLE_BUTTON_CLASS =
  "h-10 px-4 text-sm font-medium text-slate-700 bg-white border border-slate-200 rounded-xl hover:bg-slate-50 transition-all duration-150";
const PRIMARY_BUTTON_CLASS =
  "h-10 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-all duration-150 disabled:opacity-50";

// The 4 canonical Claude Code permission_mode values (feat-046 / feat-051).
// Kept as a const so the dropdown and the placeholder-default are in sync.
const PERMISSION_MODES = ["accept-edits", "default", "plan", "bypass-permissions"] as const;

// The HTTP form's sensible defaults (Anthropic today; the only HTTP
// adapter at this point per feat-039 / feat-052).
const HTTP_DEFAULTS = {
  base_url: "https://api.anthropic.com",
  default_model: "claude-sonnet-4-20250514",
};

export function AddProviderModal({ open, onClose }: AddProviderModalProps) {
  const createProvider = useCreateProvider();

  const [kind, setKind] = useState<"http" | "cli">("http");
  // HTTP fields
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState(HTTP_DEFAULTS.base_url);
  const [apiKey, setApiKey] = useState("");
  // Common
  const [defaultModel, setDefaultModel] = useState(HTTP_DEFAULTS.default_model);
  // CLI fields
  const [binaryPath, setBinaryPath] = useState("");
  const [args, setArgs] = useState<string[]>([]);
  const [env, setEnv] = useState<Array<[string, string]>>([]);
  const [permissionMode, setPermissionMode] = useState("");

  const [error, setError] = useState<string | null>(null);

  // Reset form on every open transition (mount, or false→true). Mirrors
  // the NewSessionModal pattern: don't fire on close, so a partial
  // entry survives a re-open.
  useEffect(() => {
    if (open) {
      setKind("http");
      setName("");
      setBaseUrl(HTTP_DEFAULTS.base_url);
      setApiKey("");
      setDefaultModel(HTTP_DEFAULTS.default_model);
      setBinaryPath("");
      setArgs([]);
      setEnv([]);
      setPermissionMode("");
      setError(null);
    }
  }, [open]);

  function handleClose() {
    if (createProvider.isPending) return;
    onClose();
  }

  // Switching kind: preserve name + default_model (common to both);
  // reset the variant-specific fields to per-kind defaults.
  function handleKindChange(next: "http" | "cli") {
    if (next === kind) return;
    setBaseUrl(next === "http" ? HTTP_DEFAULTS.base_url : "");
    setApiKey("");
    setBinaryPath("");
    setArgs([]);
    setEnv([]);
    setPermissionMode("");
    setKind(next);
    setError(null);
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName) {
      setError("Name is required");
      return;
    }
    // `default_model` is required by the Rust handler for BOTH kinds
    // (api/providers.rs:107-119 for http, mirrored for cli). Validate
    // up front so the user gets a friendly inline error instead of a
    // raw 400 "default_model is required" from the server.
    if (!defaultModel.trim()) {
      setError("Default model is required");
      return;
    }
    let payload: CreateProviderRequest;
    if (kind === "http") {
      if (!baseUrl.trim() || !apiKey.trim()) {
        setError("Base URL and API Key are required for HTTP providers");
        return;
      }
      payload = {
        kind: "http",
        // The legacy `type` field defaults to "anthropic" — the only
        // HTTP adapter at this point (feat-039, feat-052).
        type: "anthropic",
        name: trimmedName,
        base_url: baseUrl.trim(),
        api_key: apiKey.trim(),
        default_model: defaultModel.trim(),
      };
    } else {
      if (!binaryPath.trim()) {
        setError("Binary path is required for CLI providers");
        return;
      }
      if (!permissionMode) {
        setError("Permission mode is required for CLI providers");
        return;
      }
      payload = {
        kind: "cli",
        // The only CLI adapter at this point (feat-051); feat-058/059
        // will widen the type picker.
        type: "claude-code",
        name: trimmedName,
        binary_path: binaryPath.trim(),
        // args_json / env_json: omit the field when empty (the Rust
        // handler treats `None` and `Some("[]")` equivalently; omit is
        // cleaner on the wire and in storage).
        ...(args.length > 0 ? { args_json: JSON.stringify(args) } : {}),
        ...(env.length > 0 ? { env_json: JSON.stringify(Object.fromEntries(env)) } : {}),
        permission_mode: permissionMode,
        default_model: defaultModel.trim(),
      };
    }

    createProvider.mutate(payload, {
      onSuccess: () => {
        onClose();
      },
      onError: (err) => {
        setError(err instanceof Error ? err.message : "Failed to add provider");
      },
    });
  }

  return (
    <Modal open={open} onClose={handleClose}>
      <form onSubmit={handleSubmit} className="space-y-4" noValidate>
        <div className="flex items-center justify-between mb-6">
          <h3 className="text-lg font-semibold text-slate-900">Add Provider</h3>
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

        {/* Kind picker */}
        <div>
          <label className={LABEL_CLASS}>
            Kind <span className="text-brand-red-500">*</span>
          </label>
          <div className="grid grid-cols-2 gap-2" role="radiogroup" aria-label="Provider kind">
            <button
              type="button"
              role="radio"
              aria-checked={kind === "http"}
              onClick={() => handleKindChange("http")}
              className={
                "h-10 px-3 text-sm font-medium rounded-xl border transition-all duration-150 " +
                (kind === "http"
                  ? "bg-brand-blue-50 border-brand-blue-300 text-brand-blue-700"
                  : "bg-white border-slate-200 text-slate-700 hover:bg-slate-50")
              }
            >
              HTTP
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={kind === "cli"}
              onClick={() => handleKindChange("cli")}
              className={
                "h-10 px-3 text-sm font-medium rounded-xl border transition-all duration-150 " +
                (kind === "cli"
                  ? "bg-brand-blue-50 border-brand-blue-300 text-brand-blue-700"
                  : "bg-white border-slate-200 text-slate-700 hover:bg-slate-50")
              }
            >
              CLI
            </button>
          </div>
        </div>

        {/* Name (common) */}
        <div>
          <label className={LABEL_CLASS}>
            Name <span className="text-brand-red-500">*</span>
          </label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. Production"
            className={FIELD_CLASS}
            required
            autoFocus
          />
        </div>

        {kind === "http" ? (
          <>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className={LABEL_CLASS}>
                  Base URL <span className="text-brand-red-500">*</span>
                </label>
                <input
                  type="text"
                  value={baseUrl}
                  onChange={(e) => setBaseUrl(e.target.value)}
                  className={FIELD_CLASS}
                  required
                />
              </div>
              <div>
                <label className={LABEL_CLASS}>
                  API Key <span className="text-brand-red-500">*</span>
                </label>
                <input
                  type="password"
                  value={apiKey}
                  onChange={(e) => setApiKey(e.target.value)}
                  placeholder="sk-ant-..."
                  className={FIELD_CLASS}
                  required
                />
              </div>
            </div>
            <div>
              <label className={LABEL_CLASS}>Default Model</label>
              <input
                type="text"
                value={defaultModel}
                onChange={(e) => setDefaultModel(e.target.value)}
                className={FIELD_CLASS + " font-mono"}
                placeholder="claude-sonnet-4-20250514"
              />
            </div>
          </>
        ) : (
          <>
            <div>
              <label className={LABEL_CLASS}>
                Binary Path <span className="text-brand-red-500">*</span>
              </label>
              <input
                type="text"
                value={binaryPath}
                onChange={(e) => setBinaryPath(e.target.value)}
                placeholder="/usr/local/bin/claude"
                className={FIELD_CLASS + " font-mono"}
                required
              />
              <p className="mt-1.5 text-[11px] text-slate-500">
                Absolute path to the CLI binary on the server.
              </p>
            </div>
            <div>
              <label className={LABEL_CLASS}>Args</label>
              <ArgsEditor value={args} onChange={setArgs} />
            </div>
            <div>
              <label className={LABEL_CLASS}>Env</label>
              <EnvEditor value={env} onChange={setEnv} />
            </div>
            <div className="grid grid-cols-2 gap-4">
              <div>
                <label className={LABEL_CLASS}>
                  Permission Mode <span className="text-brand-red-500">*</span>
                </label>
                <select
                  value={permissionMode}
                  onChange={(e) => setPermissionMode(e.target.value)}
                  className={FIELD_CLASS}
                  required
                >
                  <option value="">Select…</option>
                  {PERMISSION_MODES.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
                </select>
              </div>
              <div>
                <label className={LABEL_CLASS}>Default Model</label>
                <input
                  type="text"
                  value={defaultModel}
                  onChange={(e) => setDefaultModel(e.target.value)}
                  className={FIELD_CLASS + " font-mono"}
                  placeholder="claude-sonnet-4-20250514"
                />
              </div>
            </div>
          </>
        )}

        {/* Actions */}
        <div className="flex justify-end gap-3 pt-2">
          <button
            type="button"
            onClick={handleClose}
            disabled={createProvider.isPending}
            className={SUBTLE_BUTTON_CLASS}
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={createProvider.isPending}
            className={PRIMARY_BUTTON_CLASS}
          >
            {createProvider.isPending ? "Saving…" : "Save Provider"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

// ---------------------------------------------------------------------------
// ArgsEditor — friendly list of text inputs with add/remove rows.
// Serializes the array of strings to `args_json` on submit.
// ---------------------------------------------------------------------------

function ArgsEditor({ value, onChange }: { value: string[]; onChange: (next: string[]) => void }) {
  return (
    <div className="space-y-2">
      {value.length === 0 ? (
        <p className="text-xs text-slate-400 italic">No args. Click "Add arg" to add one.</p>
      ) : (
        value.map((arg, i) => (
          <div key={i} className="flex items-center gap-2">
            <input
              type="text"
              value={arg}
              onChange={(e) => {
                const next = value.slice();
                next[i] = e.target.value;
                onChange(next);
              }}
              placeholder={`arg ${i + 1}`}
              className={FIELD_CLASS + " font-mono flex-1"}
            />
            <button
              type="button"
              onClick={() => onChange(value.filter((_, j) => j !== i))}
              aria-label={`Remove arg ${i + 1}`}
              className="h-10 w-10 text-slate-400 hover:text-brand-red-600 hover:bg-brand-red-50 rounded-xl transition-colors flex-shrink-0"
            >
              <svg
                className="w-4 h-4 mx-auto"
                fill="none"
                viewBox="0 0 24 24"
                stroke="currentColor"
                strokeWidth={2}
              >
                <line x1="18" y1="6" x2="6" y2="18" />
                <line x1="6" y1="6" x2="18" y2="18" />
              </svg>
            </button>
          </div>
        ))
      )}
      <button
        type="button"
        onClick={() => onChange([...value, ""])}
        className="h-9 px-3 text-xs font-medium text-brand-blue-600 bg-brand-blue-50 border border-brand-blue-200/60 rounded-lg hover:bg-brand-blue-100 transition-all duration-150"
      >
        + Add arg
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// EnvEditor — friendly list of key+value rows with add/remove.
// Serializes the entries to a JSON object as `env_json` on submit.
// ---------------------------------------------------------------------------

function EnvEditor({
  value,
  onChange,
}: {
  value: Array<[string, string]>;
  onChange: (next: Array<[string, string]>) => void;
}) {
  return (
    <div className="space-y-2">
      {value.length === 0 ? (
        <p className="text-xs text-slate-400 italic">
          No env vars. Click "Add env var" to add one.
        </p>
      ) : (
        <>
          <div className="grid grid-cols-[1fr_1fr_40px] gap-2 text-[10px] font-semibold uppercase tracking-[0.14em] text-slate-400">
            <span>Key</span>
            <span>Value</span>
            <span />
          </div>
          {value.map(([k, v], i) => (
            <div key={i} className="grid grid-cols-[1fr_1fr_40px] gap-2">
              <input
                type="text"
                value={k}
                onChange={(e) => {
                  const next = value.slice();
                  next[i] = [e.target.value, v];
                  onChange(next);
                }}
                placeholder="KEY"
                className={FIELD_CLASS + " font-mono"}
              />
              <input
                type="text"
                value={v}
                onChange={(e) => {
                  const next = value.slice();
                  next[i] = [k, e.target.value];
                  onChange(next);
                }}
                placeholder="value"
                className={FIELD_CLASS + " font-mono"}
              />
              <button
                type="button"
                onClick={() => onChange(value.filter((_, j) => j !== i))}
                aria-label={`Remove env var ${i + 1}`}
                className="h-10 w-10 text-slate-400 hover:text-brand-red-600 hover:bg-brand-red-50 rounded-xl transition-colors flex-shrink-0"
              >
                <svg
                  className="w-4 h-4 mx-auto"
                  fill="none"
                  viewBox="0 0 24 24"
                  stroke="currentColor"
                  strokeWidth={2}
                >
                  <line x1="18" y1="6" x2="6" y2="18" />
                  <line x1="6" y1="6" x2="18" y2="18" />
                </svg>
              </button>
            </div>
          ))}
        </>
      )}
      <button
        type="button"
        onClick={() => onChange([...value, ["", ""]])}
        className="h-9 px-3 text-xs font-medium text-brand-blue-600 bg-brand-blue-50 border border-brand-blue-200/60 rounded-lg hover:bg-brand-blue-100 transition-all duration-150"
      >
        + Add env var
      </button>
    </div>
  );
}
