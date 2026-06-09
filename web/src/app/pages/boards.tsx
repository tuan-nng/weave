// BoardsListPage — `/boards`. Lists all boards grouped by workspace, mirrors
// the structure of /sessions and /codebases. Each row links to the
// workspace-scoped board detail page at
// `/workspaces/:wid/boards/:bid`. The per-workspace section always renders
// the workspace heading AND the "+ New board in {name}" button, even when
// the workspace has zero boards — otherwise there is no entry point to
// register the first board (mirrors the post-feat-061 pattern in
// `sessions.tsx` / `codebases.tsx`).

import { useState } from "react";
import { Link, useNavigate } from "react-router";
import { NewBoardModal } from "../../components/new-board-modal";
import { Spinner } from "../../components/spinner";
import { useBoards } from "../../hooks/use-board";
import { useWorkspaces } from "../../hooks/use-workspaces";
import { ROUTES } from "../../lib/routes";
import type { Board } from "../../lib/types";

function WorkspaceBoards({
  workspaceId,
  workspaceName,
  onCreate,
}: {
  workspaceId: string;
  workspaceName: string;
  onCreate: (workspaceId: string) => void;
}) {
  const { data: boards, isLoading, error } = useBoards(workspaceId);

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
          + New board in {workspaceName}
        </button>
      </div>

      {boards && boards.length > 0 ? (
        <div className="bg-white rounded-2xl border border-slate-200/60 shadow-sm overflow-hidden">
          {boards.map((b: Board) => (
            <Link
              key={b.id}
              to={ROUTES.board(workspaceId, b.id)}
              className="block px-5 py-4 border-b border-slate-100/80 last:border-0 hover:bg-brand-blue-50/30 transition-colors"
            >
              <div className="flex items-center justify-between">
                <div className="flex-1 min-w-0">
                  <p className="text-sm font-medium text-slate-900 truncate">{b.name}</p>
                  <p className="text-xs text-slate-400 mt-0.5 font-mono">
                    {b.id.slice(0, 8)} · {new Date(b.created_at).toLocaleDateString()}
                  </p>
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
        <p className="text-sm text-slate-400 px-1">No boards yet</p>
      )}
    </div>
  );
}

export default function BoardsPage() {
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
    return (
      <div className="p-8 max-w-4xl mx-auto text-center text-sm text-slate-500">
        Failed to load boards
      </div>
    );
  }

  const hasWorkspaces = workspaces && workspaces.length > 0;

  return (
    <div className="p-8 max-w-4xl mx-auto">
      <div className="mb-8">
        <h1 className="text-2xl font-display font-semibold text-slate-900">Boards</h1>
        <p className="text-sm text-slate-500 mt-1">All kanban boards across workspaces</p>
      </div>

      {!hasWorkspaces ? (
        <div className="text-center py-16">
          <h3 className="text-sm font-medium text-slate-900 mb-1">No workspaces</h3>
          <p className="text-sm text-slate-500">Create a workspace first to start boards</p>
        </div>
      ) : (
        workspaces.map((ws) => (
          <WorkspaceBoards
            key={ws.id}
            workspaceId={ws.id}
            workspaceName={ws.name}
            onCreate={setCreateWorkspaceId}
          />
        ))
      )}

      <NewBoardModal
        workspaceId={createWorkspaceId}
        onClose={() => setCreateWorkspaceId(null)}
        onCreated={(board) => navigate(ROUTES.board(board.workspace_id, board.id))}
      />
    </div>
  );
}
