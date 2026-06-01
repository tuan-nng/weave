import { useState } from "react";
import { Link, useParams } from "react-router";
import { useCreateSession, useWorkspaceSessions, useWorkspaces } from "../../hooks/use-workspaces";
import { useProviders } from "../../hooks/use-providers";
import { ROUTES } from "../../lib/routes";
import { Modal } from "../../components/modal";
import { ErrorBanner } from "../../components/error-banner";
import { Spinner } from "../../components/spinner";
import { StatusBadge } from "../../components/status-badge";
import type { CreateSessionRequest } from "../../lib/types";

export default function WorkspacePage() {
  const { id: workspaceId } = useParams<{ id: string }>();
  const { data: workspacesResp } = useWorkspaces();
  const { data: sessionsResp, isLoading, error } = useWorkspaceSessions(workspaceId!);
  const { data: providers } = useProviders();

  const workspaces = workspacesResp?.data;
  const sessions = sessionsResp?.data;
  const createSession = useCreateSession(workspaceId!);

  const [showNewSession, setShowNewSession] = useState(false);
  const [bannerError, setBannerError] = useState<string | null>(null);
  const [form, setForm] = useState<CreateSessionRequest>({
    provider_id: "",
    specialist_id: undefined,
    model: undefined,
  });

  const workspace = workspaces?.find((w) => w.id === workspaceId);

  const handleCreateSession = (e: React.FormEvent) => {
    e.preventDefault();
    if (!form.provider_id) {
      setBannerError("Provider is required");
      return;
    }

    createSession.mutate(form, {
      onSuccess: () => {
        setShowNewSession(false);
        setForm({ provider_id: "", specialist_id: undefined, model: undefined });
      },
      onError: (err) => {
        setBannerError(err instanceof Error ? err.message : "Failed to create session");
      },
    });
  };

  if (isLoading) return <Spinner />;

  if (error) {
    return <div className="p-8 text-center text-red-600">Failed to load sessions</div>;
  }

  return (
    <div className="max-w-5xl mx-auto px-8 py-8">
      {bannerError && <ErrorBanner message={bannerError} onDismiss={() => setBannerError(null)} />}

      {/* Back navigation */}
      <Link
        to={ROUTES.home}
        className="flex items-center gap-2 text-sm text-neutral-500 hover:text-neutral-700 mb-6 transition-colors"
      >
        <svg
          className="w-4 h-4"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            d="M10.5 19.5L3 12m0 0l7.5-7.5M3 12h18"
          />
        </svg>
        Back to Workspaces
      </Link>

      {/* Header */}
      <div className="flex items-center justify-between mb-8">
        <div>
          <h1 className="text-2xl font-semibold text-neutral-900 tracking-tight">
            {workspace?.name ?? "Workspace"}
          </h1>
          <p className="mt-1 text-sm text-neutral-500">Manage sessions for this workspace</p>
        </div>
        <button
          onClick={() => setShowNewSession(true)}
          className="h-10 px-4 bg-blue-500 text-white text-sm font-medium rounded-lg hover:bg-blue-600 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 transition-colors flex items-center gap-2"
        >
          <svg
            className="w-4 h-4"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 4.5v15m7.5-7.5h-15" />
          </svg>
          New Session
        </button>
      </div>

      {/* Session table */}
      {sessions && sessions.length > 0 ? (
        <div className="bg-white rounded-xl border border-neutral-200 overflow-hidden">
          <table className="w-full">
            <thead>
              <tr className="border-b border-neutral-100">
                <th className="text-left px-5 py-3 text-xs font-medium text-neutral-500 uppercase tracking-wider">
                  Status
                </th>
                <th className="text-left px-5 py-3 text-xs font-medium text-neutral-500 uppercase tracking-wider">
                  Specialist
                </th>
                <th className="text-left px-5 py-3 text-xs font-medium text-neutral-500 uppercase tracking-wider">
                  Model
                </th>
                <th className="text-left px-5 py-3 text-xs font-medium text-neutral-500 uppercase tracking-wider">
                  Created
                </th>
              </tr>
            </thead>
            <tbody>
              {sessions.map((s) => (
                <tr
                  key={s.id}
                  className="border-b border-neutral-100 last:border-0 cursor-pointer hover:bg-neutral-50 transition-colors"
                >
                  <td className="px-5 py-3.5">
                    <StatusBadge status={s.status} />
                  </td>
                  <td className="px-5 py-3.5 text-sm text-neutral-700">
                    {s.specialist_id ?? <span className="text-neutral-400">—</span>}
                  </td>
                  <td className="px-5 py-3.5 text-sm text-neutral-500 font-mono text-xs">
                    {s.model ?? <span className="text-neutral-400">—</span>}
                  </td>
                  <td className="px-5 py-3.5 text-sm text-neutral-500 font-mono text-xs">
                    {new Date(s.created_at).toLocaleString()}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
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
                d="M8.625 12a.375.375 0 11-.75 0 .375.375 0 01.75 0zm0 0H8.25m4.125 0a.375.375 0 11-.75 0 .375.375 0 01.75 0zm0 0H12m4.125 0a.375.375 0 11-.75 0 .375.375 0 01.75 0zm0 0h-.375M21 12c0 4.556-4.03 8.25-9 8.25a9.764 9.764 0 01-2.555-.337A5.972 5.972 0 015.41 20.97a5.969 5.969 0 01-.474-.065 4.48 4.48 0 00.978-2.025c.09-.457-.133-.901-.467-1.226C3.93 16.178 3 14.189 3 12c0-4.556 4.03-8.25 9-8.25s9 3.694 9 8.25z"
              />
            </svg>
          </div>
          <h3 className="text-sm font-medium text-neutral-900 mb-1">No sessions</h3>
          <p className="text-sm text-neutral-500">
            Create a new session to start coordinating agents
          </p>
        </div>
      )}

      {/* New Session modal */}
      <Modal open={showNewSession} onClose={() => setShowNewSession(false)}>
        <div className="flex items-center justify-between mb-6">
          <h3 className="text-lg font-semibold text-neutral-900">New Session</h3>
          <button
            onClick={() => setShowNewSession(false)}
            className="text-neutral-400 hover:text-neutral-600"
          >
            <svg
              className="w-5 h-5"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>

        <form onSubmit={handleCreateSession} className="space-y-4">
          {/* Provider */}
          <div>
            <label className="block text-sm font-medium text-neutral-700 mb-1.5">
              Provider <span className="text-red-500">*</span>
            </label>
            <select
              value={form.provider_id}
              onChange={(e) => setForm((f) => ({ ...f, provider_id: e.target.value }))}
              className="w-full h-10 px-3.5 bg-white border border-neutral-200 rounded-lg text-sm text-neutral-900 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
              required
            >
              <option value="">Select provider...</option>
              {providers?.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </div>

          {/* Specialist */}
          <div>
            <label className="block text-sm font-medium text-neutral-700 mb-1.5">Specialist</label>
            <input
              type="text"
              value={form.specialist_id ?? ""}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  specialist_id: e.target.value || undefined,
                }))
              }
              placeholder="Leave empty for none"
              className="w-full h-10 px-3.5 bg-white border border-neutral-200 rounded-lg text-sm text-neutral-900 placeholder:text-neutral-400 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
            />
          </div>

          {/* Model */}
          <div>
            <label className="block text-sm font-medium text-neutral-700 mb-1.5">Model</label>
            <input
              type="text"
              value={form.model ?? ""}
              onChange={(e) =>
                setForm((f) => ({
                  ...f,
                  model: e.target.value || undefined,
                }))
              }
              placeholder="Leave empty for provider default"
              className="w-full h-10 px-3.5 bg-white border border-neutral-200 rounded-lg text-sm text-neutral-900 placeholder:text-neutral-400 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
            />
          </div>

          {/* Actions */}
          <div className="flex justify-end gap-3 pt-2">
            <button
              type="button"
              onClick={() => setShowNewSession(false)}
              className="h-10 px-4 text-sm font-medium text-neutral-700 bg-white border border-neutral-200 rounded-lg hover:bg-neutral-50 transition-colors"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={createSession.isPending}
              className="h-10 px-4 text-sm font-medium text-white bg-blue-500 rounded-lg hover:bg-blue-600 transition-colors disabled:opacity-50"
            >
              Create Session
            </button>
          </div>
        </form>
      </Modal>
    </div>
  );
}
