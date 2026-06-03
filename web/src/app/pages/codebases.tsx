// CodebasesListPage — `/codebases`. Lists all codebases grouped by
// workspace, mirrors the structure of /boards. Each row links to the
// workspace-scoped codebase detail page at
// `/workspaces/:wid/codebases/:cid`.

import { useState } from "react";
import { Link } from "react-router";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ErrorBanner } from "../../components/error-banner";
import { Modal } from "../../components/modal";
import { Spinner } from "../../components/spinner";
import { useCodebases } from "../../hooks/use-codebase";
import { useWorkspaces } from "../../hooks/use-workspaces";
import { api } from "../../lib/api";
import { queryKeys } from "../../lib/query-keys";
import { ROUTES } from "../../lib/routes";
import type { Codebase } from "../../lib/types";

function WorkspaceCodebases({
  workspaceId,
  workspaceName,
  onCreate,
}: {
  workspaceId: string;
  workspaceName: string;
  onCreate: (workspaceId: string) => void;
}) {
  const { data: codebases = [], isLoading, error } = useCodebases(workspaceId);

  if (isLoading) return null;
  if (error || codebases.length === 0) return null;

  return (
    <div className="mb-8">
      <h3 className="text-sm font-medium text-slate-500 mb-3">{workspaceName}</h3>
      <div className="bg-white rounded-2xl border border-slate-200/60 shadow-sm overflow-hidden">
        {codebases.map((cb: Codebase) => (
          <Link
            key={cb.id}
            to={ROUTES.codebase(workspaceId, cb.id)}
            className="block px-5 py-4 border-b border-slate-100/80 last:border-0 hover:bg-brand-blue-50/30 transition-colors"
          >
            <div className="flex items-center justify-between">
              <div className="flex-1 min-w-0">
                <p className="text-sm font-medium text-slate-900 truncate">{cb.label ?? cb.path}</p>
                <p className="text-xs text-slate-400 mt-0.5 font-mono truncate">{cb.path}</p>
                {cb.branch && (
                  <p className="text-[11px] text-slate-500 mt-0.5">
                    branch: <span className="font-mono">{cb.branch}</span>
                  </p>
                )}
              </div>
              <svg
                width="16"
                height="16"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth={2}
                strokeLinecap="round"
                strokeLinejoin="round"
                className="text-slate-300"
              >
                <polyline points="9 18 15 12 9 6" />
              </svg>
            </div>
          </Link>
        ))}
      </div>
      <div className="mt-3">
        <button
          type="button"
          onClick={() => onCreate(workspaceId)}
          className="h-9 px-3 text-xs font-medium text-slate-600 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 hover:border-slate-300 transition-all duration-150"
        >
          + New codebase in {workspaceName}
        </button>
      </div>
    </div>
  );
}

export default function CodebasesPage() {
  const { data: workspacesResp, isLoading, error } = useWorkspaces();
  const workspaces = workspacesResp?.data;
  const [createWorkspaceId, setCreateWorkspaceId] = useState<string | null>(null);
  const [bannerError, setBannerError] = useState<string | null>(null);

  return (
    <div className="p-8 max-w-4xl mx-auto">
      <div className="mb-8">
        <h1 className="text-2xl font-display font-semibold text-slate-900">Codebases</h1>
        <p className="text-sm text-slate-500 mt-1">Git repositories registered across workspaces</p>
      </div>

      {bannerError && (
        <div className="mb-6">
          <ErrorBanner message={bannerError} onDismiss={() => setBannerError(null)} />
        </div>
      )}

      {isLoading ? (
        <div className="flex items-center justify-center h-64">
          <Spinner />
        </div>
      ) : error ? (
        <ErrorBanner message="Failed to load workspaces" onDismiss={() => {}} />
      ) : !workspaces || workspaces.length === 0 ? (
        <div className="rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm p-12 text-center animate-fade-in-up">
          <div className="w-12 h-12 bg-brand-slate-100 rounded-xl flex items-center justify-center mx-auto mb-4">
            <svg
              width="24"
              height="24"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeLinecap="round"
              strokeLinejoin="round"
              className="text-slate-400"
            >
              <circle cx="6" cy="6" r="1.5" />
              <circle cx="6" cy="18" r="1.5" />
              <circle cx="18" cy="6" r="1.5" />
              <path d="M6 7.5v9" />
              <path d="M7.5 6h4a3 3 0 013 3v0" />
            </svg>
          </div>
          <h3 className="text-sm font-medium text-slate-900 mb-1">No workspaces</h3>
          <p className="text-sm text-slate-500">Create a workspace first to register codebases</p>
        </div>
      ) : (
        workspaces.map((ws) => (
          <WorkspaceCodebases
            key={ws.id}
            workspaceId={ws.id}
            workspaceName={ws.name}
            onCreate={(wid) => setCreateWorkspaceId(wid)}
          />
        ))
      )}

      <CreateCodebaseModal
        workspaceId={createWorkspaceId}
        onClose={() => setCreateWorkspaceId(null)}
        onError={setBannerError}
      />
    </div>
  );
}

function CreateCodebaseModal({
  workspaceId,
  onClose,
  onError,
}: {
  workspaceId: string | null;
  onClose: () => void;
  onError: (msg: string) => void;
}) {
  const qc = useQueryClient();
  const [path, setPath] = useState("");
  const [label, setLabel] = useState("");

  const createMutation = useMutation({
    mutationFn: ({ wid, path, label }: { wid: string; path: string; label: string | null }) =>
      api.codebases.create(wid, { path, label: label ?? undefined }),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: queryKeys.codebases.list(vars.wid) });
      setPath("");
      setLabel("");
      onClose();
    },
    onError: (err: unknown) => {
      onError(err instanceof Error ? err.message : "Failed to create codebase");
    },
  });

  const submit = () => {
    if (!workspaceId) return;
    const trimmed = path.trim();
    if (trimmed.length === 0) {
      onError("Path is required");
      return;
    }
    createMutation.mutate({
      wid: workspaceId,
      path: trimmed,
      label: label.trim() || null,
    });
  };

  return (
    <Modal open={workspaceId !== null} onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
        className="space-y-4"
      >
        <div className="flex items-center justify-between mb-6">
          <h3 className="text-lg font-semibold text-slate-900">New codebase</h3>
          <button
            type="button"
            onClick={onClose}
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
            Path <span className="text-brand-red-500">*</span>
          </label>
          <input
            type="text"
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="/Users/me/projects/my-app"
            autoFocus
            className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm font-mono text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
          />
          <p className="text-[11px] text-slate-500 mt-1.5">
            Absolute path to a git working tree on disk.
          </p>
        </div>

        <div>
          <label className="block text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-1.5">
            Label
          </label>
          <input
            type="text"
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="e.g. Backend, Mobile"
            className="w-full h-10 px-3.5 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
          />
        </div>

        <div className="flex items-center justify-end gap-2 pt-2">
          <button
            type="button"
            onClick={onClose}
            className="h-9 px-3 text-sm font-medium text-slate-700 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 transition-all duration-150"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={path.trim().length === 0 || createMutation.isPending}
            className="h-9 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-all duration-150 disabled:opacity-50"
          >
            {createMutation.isPending ? "Creating…" : "Create codebase"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
