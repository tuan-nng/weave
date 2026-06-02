// BoardsListPage — `/boards`. Lists all boards grouped by workspace,
// mirrors the structure of /sessions (which lists sessions per workspace).
// Each row links to the workspace-scoped board detail page.

import { useState } from "react";
import { Link } from "react-router";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ErrorBanner } from "../../components/error-banner";
import { Modal } from "../../components/modal";
import { Spinner } from "../../components/spinner";
import { useBoards } from "../../hooks/use-board";
import { useWorkspaces } from "../../hooks/use-workspaces";
import { api } from "../../lib/api";
import { queryKeys } from "../../lib/query-keys";
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
  const { data: boards = [], isLoading, error } = useBoards(workspaceId);

  if (isLoading) return null;
  if (error || boards.length === 0) return null;

  return (
    <div className="mb-8">
      <h3 className="text-sm font-medium text-slate-500 mb-3">{workspaceName}</h3>
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
      <div className="mt-3">
        <button
          type="button"
          onClick={() => onCreate(workspaceId)}
          className="h-9 px-3 text-xs font-medium text-slate-600 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 hover:border-slate-300 transition-all duration-150"
        >
          + New board in {workspaceName}
        </button>
      </div>
    </div>
  );
}

export default function BoardsPage() {
  const { data: workspacesResp, isLoading, error } = useWorkspaces();
  const workspaces = workspacesResp?.data;
  const [createWorkspaceId, setCreateWorkspaceId] = useState<string | null>(null);
  const [bannerError, setBannerError] = useState<string | null>(null);

  return (
    <div className="p-8 max-w-4xl mx-auto">
      <div className="mb-8">
        <h1 className="text-2xl font-display font-semibold text-slate-900">Boards</h1>
        <p className="text-sm text-slate-500 mt-1">All kanban boards across workspaces</p>
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
              <rect x="3" y="3" width="7" height="7" />
              <rect x="14" y="3" width="7" height="7" />
              <rect x="14" y="14" width="7" height="7" />
              <rect x="3" y="14" width="7" height="7" />
            </svg>
          </div>
          <h3 className="text-sm font-medium text-slate-900 mb-1">No workspaces</h3>
          <p className="text-sm text-slate-500">Create a workspace first to start boards</p>
        </div>
      ) : (
        workspaces.map((ws) => (
          <WorkspaceBoards
            key={ws.id}
            workspaceId={ws.id}
            workspaceName={ws.name}
            onCreate={(wid) => setCreateWorkspaceId(wid)}
          />
        ))
      )}

      <CreateBoardModal
        workspaceId={createWorkspaceId}
        onClose={() => setCreateWorkspaceId(null)}
        onError={setBannerError}
      />
    </div>
  );
}

function CreateBoardModal({
  workspaceId,
  onClose,
  onError,
}: {
  workspaceId: string | null;
  onClose: () => void;
  onError: (msg: string) => void;
}) {
  const qc = useQueryClient();
  const [name, setName] = useState("");

  const createMutation = useMutation({
    mutationFn: ({ wid, name }: { wid: string; name: string }) =>
      api.kanban.boards.create(wid, { name }),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: queryKeys.boards.list(vars.wid) });
      setName("");
      onClose();
    },
    onError: (err: unknown) => {
      onError(err instanceof Error ? err.message : "Failed to create board");
    },
  });

  return (
    <Modal open={workspaceId !== null} onClose={onClose}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!workspaceId || name.trim().length === 0) return;
          createMutation.mutate({ wid: workspaceId, name: name.trim() });
        }}
        className="space-y-4"
      >
        <div className="flex items-center justify-between mb-6">
          <h3 className="text-lg font-semibold text-slate-900">New board</h3>
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
            Name <span className="text-brand-red-500">*</span>
          </label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. Product Sprint Q3"
            autoFocus
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
            disabled={name.trim().length === 0 || createMutation.isPending}
            className="h-9 px-4 text-sm font-medium text-white bg-brand-blue-500 rounded-xl hover:bg-brand-blue-600 transition-all duration-150 disabled:opacity-50"
          >
            {createMutation.isPending ? "Creating…" : "Create board"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
