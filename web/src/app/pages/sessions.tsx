import { Link } from "react-router";
import { useWorkspaces } from "../../hooks/use-workspaces";
import { useWorkspaceSessions } from "../../hooks/use-workspaces";
import { ROUTES } from "../../lib/routes";
import { ErrorBanner } from "../../components/error-banner";
import { Spinner } from "../../components/spinner";
import { StatusBadge } from "../../components/status-badge";
import type { Session } from "../../lib/types";

function WorkspaceSessions({
  workspaceId,
  workspaceName,
}: {
  workspaceId: string;
  workspaceName: string;
}) {
  const { data: sessionsResp, isLoading, error } = useWorkspaceSessions(workspaceId);
  const sessions = sessionsResp?.data;

  if (isLoading) return null;
  if (error || !sessions || sessions.length === 0) return null;

  return (
    <div className="mb-8">
      <h3 className="text-sm font-medium text-slate-500 mb-3">{workspaceName}</h3>
      <div className="bg-white rounded-2xl border border-slate-200/60 shadow-sm overflow-hidden">
        {sessions.map((s: Session) => (
          <Link
            key={s.id}
            to={ROUTES.session(s.id)}
            className="block px-5 py-4 border-b border-slate-100/80 last:border-0 hover:bg-brand-blue-50/30 transition-colors"
          >
            <div className="flex items-center justify-between">
              <div className="flex-1 min-w-0">
                <p className="text-sm font-medium text-slate-900 truncate">
                  {s.title || `Session ${s.id.slice(0, 8)}`}
                </p>
                <p className="text-xs text-slate-400 mt-0.5">
                  {new Date(s.created_at).toLocaleDateString()} · {s.model || "default model"}
                </p>
              </div>
              <StatusBadge status={s.status} />
            </div>
          </Link>
        ))}
      </div>
    </div>
  );
}

export default function SessionsPage() {
  const { data: workspacesResp, isLoading, error } = useWorkspaces();
  const workspaces = workspacesResp?.data;

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner />
      </div>
    );
  }

  if (error) {
    return <ErrorBanner message="Failed to load sessions" />;
  }

  const hasWorkspaces = workspaces && workspaces.length > 0;

  return (
    <div className="p-8 max-w-4xl mx-auto">
      <div className="mb-8">
        <h1 className="text-2xl font-display font-semibold text-slate-900">Sessions</h1>
        <p className="text-sm text-slate-500 mt-1">All agent sessions across workspaces</p>
      </div>

      {!hasWorkspaces ? (
        <div className="text-center py-16">
          <h3 className="text-sm font-medium text-slate-900 mb-1">No workspaces</h3>
          <p className="text-sm text-slate-500">Create a workspace first to start sessions</p>
        </div>
      ) : (
        workspaces.map((ws) => (
          <WorkspaceSessions key={ws.id} workspaceId={ws.id} workspaceName={ws.name} />
        ))
      )}
    </div>
  );
}
