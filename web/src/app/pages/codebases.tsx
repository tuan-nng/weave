// CodebasesListPage — `/codebases`. Lists all codebases grouped by
// workspace, mirrors the structure of /sessions. Each row links to the
// workspace-scoped codebase detail page at
// `/workspaces/:wid/codebases/:cid`. The per-workspace section always
// renders the workspace heading AND the "+ New codebase in {name}"
// button, even when the workspace has zero codebases — otherwise there
// is no entry point to register the first codebase (mirrors the
// post-feat-061 pattern in `sessions.tsx`).

import { useState } from "react";
import { Link, useNavigate } from "react-router";
import { ErrorBanner } from "../../components/error-banner";
import { NewCodebaseModal } from "../../components/new-codebase-modal";
import { Spinner } from "../../components/spinner";
import { useCodebases } from "../../hooks/use-codebase";
import { useWorkspaces } from "../../hooks/use-workspaces";
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
  const { data: codebases, isLoading, error } = useCodebases(workspaceId);

  if (isLoading) return null;
  if (error) return null;

  return (
    <div className="mb-8">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-sm font-medium text-slate-500">{workspaceName}</h3>
        <button
          type="button"
          onClick={() => onCreate(workspaceId)}
          className="h-9 px-3 text-xs font-medium text-slate-600 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 hover:border-slate-300 transition-all duration-150"
        >
          + New codebase in {workspaceName}
        </button>
      </div>

      {codebases && codebases.length > 0 ? (
        <div className="bg-white rounded-2xl border border-slate-200/60 shadow-sm overflow-hidden">
          {codebases.map((cb: Codebase) => (
            <Link
              key={cb.id}
              to={ROUTES.codebase(workspaceId, cb.id)}
              className="block px-5 py-4 border-b border-slate-100/80 last:border-0 hover:bg-brand-blue-50/30 transition-colors"
            >
              <div className="flex items-center justify-between">
                <div className="flex-1 min-w-0">
                  <p className="text-sm font-medium text-slate-900 truncate">
                    {cb.label ?? cb.path}
                  </p>
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
      ) : (
        <p className="text-sm text-slate-400 px-1">No codebases yet</p>
      )}
    </div>
  );
}

export default function CodebasesPage() {
  const { data: workspacesResp, isLoading, error } = useWorkspaces();
  const workspaces = workspacesResp?.data;
  const [createWorkspaceId, setCreateWorkspaceId] = useState<string | null>(null);
  const navigate = useNavigate();

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner />
      </div>
    );
  }

  if (error) {
    return <ErrorBanner message="Failed to load codebases" onDismiss={() => {}} />;
  }

  const hasWorkspaces = workspaces && workspaces.length > 0;

  return (
    <div className="p-8 max-w-4xl mx-auto">
      <div className="mb-8">
        <h1 className="text-2xl font-display font-semibold text-slate-900">Codebases</h1>
        <p className="text-sm text-slate-500 mt-1">Git repositories registered across workspaces</p>
      </div>

      {!hasWorkspaces ? (
        <div className="text-center py-16">
          <h3 className="text-sm font-medium text-slate-900 mb-1">No workspaces</h3>
          <p className="text-sm text-slate-500">Create a workspace first to register codebases</p>
        </div>
      ) : (
        workspaces.map((ws) => (
          <WorkspaceCodebases
            key={ws.id}
            workspaceId={ws.id}
            workspaceName={ws.name}
            onCreate={setCreateWorkspaceId}
          />
        ))
      )}

      <NewCodebaseModal
        workspaceId={createWorkspaceId}
        onClose={() => setCreateWorkspaceId(null)}
        onCreated={(cb) => navigate(ROUTES.codebase(cb.workspace_id, cb.id))}
      />
    </div>
  );
}
