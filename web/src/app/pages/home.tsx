import { useState } from "react";
import { Link } from "react-router";
import {
  useCreateWorkspace,
  useDeleteWorkspace,
  useRenameWorkspace,
  useWorkspaces,
} from "../../hooks/use-workspaces";
import { useProviders } from "../../hooks/use-providers";
import { ROUTES } from "../../lib/routes";
import { Modal } from "../../components/modal";
import { ErrorBanner } from "../../components/error-banner";
import { Spinner } from "../../components/spinner";

const WORKSPACE_COLORS = [
  { from: "from-brand-blue-50", to: "to-brand-blue-100", border: "border-brand-blue-200/60", icon: "text-brand-blue-500" },
  { from: "from-brand-orchid-50", to: "to-brand-orchid-100", border: "border-brand-orchid-200/60", icon: "text-brand-orchid-500" },
  { from: "from-brand-emerald-50", to: "to-brand-emerald-100", border: "border-brand-emerald-200/60", icon: "text-brand-emerald-500" },
  { from: "from-brand-amber-50", to: "to-brand-amber-100", border: "border-brand-amber-200/60", icon: "text-brand-amber-500" },
];

function WorkspaceIcon({ colorIndex }: { colorIndex: number }) {
  const c = WORKSPACE_COLORS[colorIndex % WORKSPACE_COLORS.length];
  return (
    <div className={`w-10 h-10 rounded-xl bg-gradient-to-br ${c.from} ${c.to} border ${c.border} flex items-center justify-center flex-shrink-0`}>
      <svg className={`w-5 h-5 ${c.icon}`} fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1.8}>
        <path strokeLinecap="round" strokeLinejoin="round" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z" />
      </svg>
    </div>
  );
}

export default function HomePage() {
  const { data: workspacesResp, isLoading, error } = useWorkspaces();
  const workspaces = workspacesResp?.data;
  const createWs = useCreateWorkspace();
  const renameWs = useRenameWorkspace();
  const deleteWs = useDeleteWorkspace();
  const { data: providers } = useProviders();

  const [newName, setNewName] = useState("");
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [deleteTarget, setDeleteTarget] = useState<{
    id: string;
    name: string;
  } | null>(null);
  const [bannerError, setBannerError] = useState<string | null>(null);

  const handleCreate = (e: React.FormEvent) => {
    e.preventDefault();
    const name = newName.trim();
    if (!name) return;

    if (workspaces?.some((w) => w.name.toLowerCase() === name.toLowerCase())) {
      setBannerError("A workspace with this name already exists");
      return;
    }

    createWs.mutate(
      { name },
      {
        onSuccess: () => setNewName(""),
        onError: (err) => setBannerError(err instanceof Error ? err.message : "Create failed"),
      },
    );
  };

  const handleRename = (id: string) => {
    const value = renameValue.trim();
    if (!value) {
      setRenamingId(null);
      return;
    }

    renameWs.mutate(
      { id, name: value },
      {
        onSuccess: () => setRenamingId(null),
        onError: (err) => {
          setBannerError(err instanceof Error ? err.message : "Rename failed");
          setRenamingId(null);
        },
      },
    );
  };

  const handleDelete = () => {
    if (!deleteTarget) return;
    deleteWs.mutate(deleteTarget.id, {
      onSuccess: () => setDeleteTarget(null),
      onError: (err) => {
        setBannerError(err instanceof Error ? err.message : "Delete failed");
        setDeleteTarget(null);
      },
    });
  };

  if (isLoading) return <Spinner />;

  if (error) {
    return <div className="p-8 text-center text-brand-red-600">Failed to load workspaces</div>;
  }

  return (
    <div className="max-w-5xl mx-auto px-8 py-8 lg:px-12 lg:py-10">
      {bannerError && <ErrorBanner message={bannerError} onDismiss={() => setBannerError(null)} />}

      {/* Header */}
      <div className="mb-8 animate-fade-in">
        <h1 className="font-display text-2xl font-semibold tracking-tight text-slate-900">Workspaces</h1>
        <p className="mt-1.5 text-sm text-slate-500">Manage your agent coordination workspaces</p>
      </div>

      {/* Create form */}
      <div className="mb-8 animate-fade-in-up" style={{ animationDelay: "50ms" }}>
        <div className="rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm p-5 shadow-[0_1px_3px_rgba(0,0,0,0.04)] hover:shadow-[0_4px_12px_rgba(0,0,0,0.06)] transition-shadow duration-200">
          <form onSubmit={handleCreate} className="flex items-center gap-3">
            <div className="relative flex-1">
              <svg className="absolute left-3.5 top-1/2 -translate-y-1/2 w-4 h-4 text-slate-400" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}>
                <path strokeLinecap="round" strokeLinejoin="round" d="M12 6v6m0 0v6m0-6h6m-6 0H6" />
              </svg>
              <input
                type="text"
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                placeholder="Enter workspace name..."
                className="w-full h-10 pl-10 pr-4 bg-white border border-slate-200 rounded-xl text-sm text-slate-900 placeholder:text-slate-400 focus:outline-none focus:ring-2 focus:ring-brand-blue-500/30 focus:border-brand-blue-400 transition-all duration-150"
                required
              />
            </div>
            <button
              type="submit"
              disabled={createWs.isPending}
              className="h-10 px-5 bg-brand-blue-500 text-white text-sm font-medium rounded-xl hover:bg-brand-blue-600 focus:outline-none focus:ring-2 focus:ring-brand-blue-500 focus:ring-offset-2 transition-all duration-150 shadow-sm hover:shadow-md disabled:opacity-50"
            >
              Create
            </button>
          </form>
        </div>
      </div>

      {/* Workspace list */}
      {workspaces && workspaces.length > 0 ? (
        <div className="space-y-3 animate-fade-in-up" style={{ animationDelay: "100ms" }}>
          {workspaces.map((ws, index) => (
            <div
              key={ws.id}
              className="group block rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm p-5 shadow-[0_1px_3px_rgba(0,0,0,0.04)] hover:shadow-[0_4px_12px_rgba(0,0,0,0.06)] hover:border-brand-blue-200 hover:bg-white transition-all duration-200"
            >
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-4 min-w-0">
                  <WorkspaceIcon colorIndex={ws.is_default ? 0 : (index % (WORKSPACE_COLORS.length - 1)) + 1} />
                  <div className="min-w-0">
                    <div className="flex items-center gap-2.5">
                      {renamingId === ws.id ? (
                        <input
                          type="text"
                          value={renameValue}
                          onChange={(e) => setRenameValue(e.target.value)}
                          onBlur={() => handleRename(ws.id)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter") handleRename(ws.id);
                            if (e.key === "Escape") setRenamingId(null);
                          }}
                          autoFocus
                          className="text-sm font-semibold text-slate-900 bg-transparent border-b border-brand-blue-400 outline-none"
                          style={{ minWidth: 120 }}
                        />
                      ) : (
                        <Link
                          to={ROUTES.workspace(ws.id)}
                          className="text-sm font-semibold text-slate-900 hover:text-brand-blue-600 transition-colors truncate"
                        >
                          {ws.name}
                        </Link>
                      )}
                      {ws.is_default && (
                        <span className="inline-flex items-center px-2 py-0.5 rounded-full text-[10px] font-semibold tracking-wide bg-brand-blue-50 text-brand-blue-700 border border-brand-blue-200/60 uppercase">
                          ★ Default
                        </span>
                      )}
                    </div>
                    <p className="text-xs text-slate-400 mt-0.5 font-mono">
                      Created {new Date(ws.created_at).toLocaleDateString()}
                    </p>
                  </div>
                </div>
                <div className="flex items-center gap-2 opacity-0 group-hover:opacity-100 transition-opacity duration-150">
                  {!ws.is_default && (
                    <>
                      <button
                        onClick={() => {
                          setRenamingId(ws.id);
                          setRenameValue(ws.name);
                        }}
                        className="h-8 px-3 text-xs font-medium text-slate-600 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 hover:border-slate-300 transition-all duration-150"
                      >
                        Rename
                      </button>
                      <button
                        onClick={() => setDeleteTarget({ id: ws.id, name: ws.name })}
                        className="h-8 px-3 text-xs font-medium text-brand-red-600 bg-brand-red-50 border border-brand-red-200/60 rounded-lg hover:bg-brand-red-100 transition-all duration-150"
                      >
                        Delete
                      </button>
                    </>
                  )}
                </div>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm p-12 text-center animate-fade-in-up" style={{ animationDelay: "100ms" }}>
          <div className="w-12 h-12 bg-brand-slate-100 rounded-xl flex items-center justify-center mx-auto mb-4">
            <svg
              className="w-6 h-6 text-slate-400"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={1.5}
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M2.25 12.75V12A2.25 2.25 0 014.5 9.75h15A2.25 2.25 0 0121.75 12v.75m-8.69-6.44l-2.12-2.12a1.5 1.5 0 00-1.061-.44H4.5A2.25 2.25 0 002.25 6v12a2.25 2.25 0 002.25 2.25h15A2.25 2.25 0 0021.75 18V9a2.25 2.25 0 00-2.25-2.25h-5.379a1.5 1.5 0 01-1.06-.44z"
              />
            </svg>
          </div>
          <h3 className="text-sm font-medium text-slate-900 mb-1">No workspaces</h3>
          <p className="text-sm text-slate-500">Create your first workspace to get started</p>
        </div>
      )}

      {/* Stats Row */}
      {workspaces && workspaces.length > 0 && (
        <div className="mt-10 grid grid-cols-3 gap-4 animate-fade-in-up" style={{ animationDelay: "150ms" }}>
          <div className="rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm px-5 py-4">
            <p className="text-[11px] font-medium uppercase tracking-[0.16em] text-slate-400 mb-1">Workspaces</p>
            <p className="text-2xl font-display font-semibold text-slate-900">{workspaces.length}</p>
          </div>
          <div className="rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm px-5 py-4">
            <p className="text-[11px] font-medium uppercase tracking-[0.16em] text-slate-400 mb-1">Providers</p>
            <p className="text-2xl font-display font-semibold text-brand-blue-600">{providers?.length ?? 0}</p>
          </div>
          <div className="rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm px-5 py-4">
            <p className="text-[11px] font-medium uppercase tracking-[0.16em] text-slate-400 mb-1">Status</p>
            <p className="text-2xl font-display font-semibold text-brand-emerald-600">Ready</p>
          </div>
        </div>
      )}

      {/* Delete confirmation modal */}
      <Modal open={!!deleteTarget} onClose={() => setDeleteTarget(null)}>
        <div className="flex items-start gap-4 mb-6">
          <div className="w-10 h-10 bg-brand-red-50 rounded-full flex items-center justify-center flex-shrink-0">
            <svg
              className="w-5 h-5 text-brand-red-500"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M12 9v3.75m-9.303 3.376c-.866 1.5.217 3.374 1.948 3.374h14.71c1.73 0 2.813-1.874 1.948-3.374L13.949 3.378c-.866-1.5-3.032-1.5-3.898 0L2.697 16.126zM12 15.75h.007v.008H12v-.008z"
              />
            </svg>
          </div>
          <div>
            <h3 className="text-base font-semibold text-slate-900">Confirm deletion</h3>
            <p className="mt-1 text-sm text-slate-500">
              Are you sure you want to delete "{deleteTarget?.name}"? All sessions will be removed.
            </p>
          </div>
        </div>
        <div className="flex justify-end gap-3">
          <button
            onClick={() => setDeleteTarget(null)}
            className="h-10 px-4 text-sm font-medium text-slate-700 bg-white border border-slate-200 rounded-xl hover:bg-slate-50 transition-all duration-150"
          >
            Cancel
          </button>
          <button
            onClick={handleDelete}
            disabled={deleteWs.isPending}
            className="h-10 px-4 text-sm font-medium text-white bg-brand-red-500 rounded-xl hover:bg-brand-red-600 transition-all duration-150 disabled:opacity-50"
          >
            Delete
          </button>
        </div>
      </Modal>
    </div>
  );
}
