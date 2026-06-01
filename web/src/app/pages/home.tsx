import { useState } from "react";
import { Link } from "react-router";
import {
  useCreateWorkspace,
  useDeleteWorkspace,
  useRenameWorkspace,
  useWorkspaces,
} from "../../hooks/use-workspaces";
import { ROUTES } from "../../lib/routes";
import { Modal } from "../../components/modal";
import { ErrorBanner } from "../../components/error-banner";
import { Spinner } from "../../components/spinner";

export default function HomePage() {
  const { data: workspacesResp, isLoading, error } = useWorkspaces();
  const workspaces = workspacesResp?.data;
  const createWs = useCreateWorkspace();
  const renameWs = useRenameWorkspace();
  const deleteWs = useDeleteWorkspace();

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
    return <div className="p-8 text-center text-red-600">Failed to load workspaces</div>;
  }

  return (
    <div className="max-w-4xl mx-auto px-8 py-8">
      {bannerError && <ErrorBanner message={bannerError} onDismiss={() => setBannerError(null)} />}

      {/* Header */}
      <div className="mb-8">
        <h1 className="text-2xl font-semibold text-neutral-900 tracking-tight">Workspaces</h1>
        <p className="mt-1 text-sm text-neutral-500">Manage your agent coordination workspaces</p>
      </div>

      {/* Create form */}
      <div className="bg-white rounded-xl border border-neutral-200 p-5 mb-6">
        <form onSubmit={handleCreate} className="flex gap-3">
          <input
            type="text"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            placeholder="Enter workspace name..."
            className="flex-1 h-10 px-3.5 bg-neutral-50 border border-neutral-200 rounded-lg text-sm text-neutral-900 placeholder:text-neutral-400 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
            required
          />
          <button
            type="submit"
            disabled={createWs.isPending}
            className="h-10 px-4 bg-blue-500 text-white text-sm font-medium rounded-lg hover:bg-blue-600 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 transition-colors disabled:opacity-50"
          >
            Create
          </button>
        </form>
      </div>

      {/* Workspace list */}
      {workspaces && workspaces.length > 0 ? (
        <div className="space-y-3">
          {workspaces.map((ws) => (
            <div
              key={ws.id}
              className="bg-white rounded-xl border border-neutral-200 p-4 flex items-center gap-4 group hover:border-neutral-300 transition-colors"
            >
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-3">
                  {ws.is_default && (
                    <span className="inline-flex items-center gap-1.5 px-2.5 py-1 bg-blue-50 text-blue-700 text-xs font-medium rounded-md">
                      ★ Default
                    </span>
                  )}
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
                      className="inline-edit text-sm font-medium"
                      style={{ minWidth: 120 }}
                    />
                  ) : (
                    <Link
                      to={ROUTES.workspace(ws.id)}
                      className="text-sm font-medium text-neutral-900 hover:text-blue-600 transition-colors"
                    >
                      {ws.name}
                    </Link>
                  )}
                </div>
                <p className="mt-1 text-xs text-neutral-500 font-mono">
                  Created {new Date(ws.created_at).toLocaleDateString()}
                </p>
              </div>

              <div className="flex items-center gap-2 opacity-0 group-hover:opacity-100 transition-opacity">
                {!ws.is_default && (
                  <>
                    <button
                      onClick={() => {
                        setRenamingId(ws.id);
                        setRenameValue(ws.name);
                      }}
                      className="h-8 px-3 text-xs font-medium text-neutral-600 bg-neutral-50 border border-neutral-200 rounded-md hover:bg-neutral-100 transition-colors"
                    >
                      Rename
                    </button>
                    <button
                      onClick={() => setDeleteTarget({ id: ws.id, name: ws.name })}
                      className="h-8 px-3 text-xs font-medium text-red-600 bg-red-50 border border-red-200 rounded-md hover:bg-red-100 transition-colors"
                    >
                      Delete
                    </button>
                  </>
                )}
              </div>
            </div>
          ))}
        </div>
      ) : (
        <div className="bg-white rounded-xl border border-neutral-200 p-12 text-center">
          <div className="w-12 h-12 bg-neutral-100 rounded-xl flex items-center justify-center mx-auto mb-4">
            <svg
              className="w-6 h-6 text-neutral-400"
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
          <h3 className="text-sm font-medium text-neutral-900 mb-1">No workspaces</h3>
          <p className="text-sm text-neutral-500">Create your first workspace to get started</p>
        </div>
      )}

      {/* Delete confirmation modal */}
      <Modal open={!!deleteTarget} onClose={() => setDeleteTarget(null)}>
        <div className="flex items-start gap-4 mb-6">
          <div className="w-10 h-10 bg-red-50 rounded-full flex items-center justify-center flex-shrink-0">
            <svg
              className="w-5 h-5 text-red-500"
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
            <h3 className="text-base font-semibold text-neutral-900">Confirm deletion</h3>
            <p className="mt-1 text-sm text-neutral-500">
              Are you sure you want to delete "{deleteTarget?.name}"? All sessions will be removed.
            </p>
          </div>
        </div>
        <div className="flex justify-end gap-3">
          <button
            onClick={() => setDeleteTarget(null)}
            className="h-10 px-4 text-sm font-medium text-neutral-700 bg-white border border-neutral-200 rounded-lg hover:bg-neutral-50 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleDelete}
            disabled={deleteWs.isPending}
            className="h-10 px-4 text-sm font-medium text-white bg-red-500 rounded-lg hover:bg-red-600 transition-colors disabled:opacity-50"
          >
            Delete
          </button>
        </div>
      </Modal>
    </div>
  );
}
