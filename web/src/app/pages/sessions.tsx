import { useState } from "react";
import { Link, useNavigate } from "react-router";
import { useWorkspaces, useWorkspaceSessions } from "../../hooks/use-workspaces";
import { NewSessionModal } from "../../components/new-session-modal";
import { ROUTES } from "../../lib/routes";
import { ErrorBanner } from "../../components/error-banner";
import { Spinner } from "../../components/spinner";
import { StatusBadge } from "../../components/status-badge";
import type { Session } from "../../lib/types";

function WorkspaceSessions({
  workspaceId,
  workspaceName,
  onCreateSession,
}: {
  workspaceId: string;
  workspaceName: string;
  onCreateSession: (workspaceId: string) => void;
}) {
  const { data: sessionsResp, isLoading, error } = useWorkspaceSessions(workspaceId);
  const sessions = sessionsResp?.data;

  if (isLoading) return null;
  if (error) return null;

  return (
    <div className="mb-8">
      <div className="flex items-center justify-between mb-3">
        <h3 className="text-sm font-medium text-slate-500">{workspaceName}</h3>
        <button
          type="button"
          onClick={() => onCreateSession(workspaceId)}
          className="h-9 px-3 text-xs font-medium text-slate-600 bg-white border border-slate-200 rounded-lg hover:bg-slate-50 hover:border-slate-300 transition-all duration-150"
        >
          + New Session in {workspaceName}
        </button>
      </div>

      {sessions && sessions.length > 0 ? (
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
                    {s.specialist_id || `Session ${s.id.slice(0, 8)}`}
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
      ) : (
        <p className="text-sm text-slate-400 px-1">No sessions yet</p>
      )}
    </div>
  );
}

export default function SessionsPage() {
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
    return <ErrorBanner message="Failed to load sessions" onDismiss={() => {}} />;
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
          <WorkspaceSessions
            key={ws.id}
            workspaceId={ws.id}
            workspaceName={ws.name}
            onCreateSession={setCreateWorkspaceId}
          />
        ))
      )}

      <NewSessionModal
        workspaceId={createWorkspaceId}
        onClose={() => setCreateWorkspaceId(null)}
        onCreated={(session) => navigate(ROUTES.session(session.id))}
      />
    </div>
  );
}
