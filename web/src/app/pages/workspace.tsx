import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { useWorkspaceSessions, useWorkspaces } from "../../hooks/use-workspaces";
import { ROUTES } from "../../lib/routes";
import { NewSessionWizard } from "../../components/new-session-wizard";
import { Spinner } from "../../components/spinner";
import { StatusBadge } from "../../components/status-badge";

export default function WorkspacePage() {
  const { id: workspaceId } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const { data: workspacesResp } = useWorkspaces();
  const { data: sessionsResp, isLoading, error } = useWorkspaceSessions(workspaceId!);

  const workspaces = workspacesResp?.data;
  const sessions = sessionsResp?.data;

  const [showNewSession, setShowNewSession] = useState(false);

  const workspace = workspaces?.find((w) => w.id === workspaceId);

  if (isLoading) return <Spinner />;

  if (error) {
    return <div className="p-8 text-center text-brand-red-600">Failed to load sessions</div>;
  }

  return (
    <div className="max-w-6xl mx-auto px-8 py-8 lg:px-12 lg:py-10">
      {/* Back navigation */}
      <Link
        to={ROUTES.home}
        className="inline-flex items-center gap-1.5 text-sm text-slate-500 hover:text-brand-blue-600 transition-colors duration-150 mb-6 group"
      >
        <svg
          className="w-4 h-4 transition-transform duration-150 group-hover:-translate-x-0.5"
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M15 19l-7-7 7-7" />
        </svg>
        Back to Workspaces
      </Link>

      {/* Header */}
      <div className="flex items-center justify-between mb-8 animate-fade-in">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 rounded-xl bg-gradient-to-br from-brand-blue-50 to-brand-blue-100 border border-brand-blue-200/60 flex items-center justify-center">
            <svg
              className="w-5 h-5 text-brand-blue-500"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={1.8}
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"
              />
            </svg>
          </div>
          <div>
            <h1 className="font-display text-2xl font-semibold tracking-tight text-slate-900">
              {workspace?.name ?? "Workspace"}
            </h1>
            {workspaceId && (
              <p className="text-xs text-slate-400 font-mono mt-0.5">{workspaceId}</p>
            )}
          </div>
        </div>
        <button
          onClick={() => setShowNewSession(true)}
          className="h-10 px-5 bg-brand-blue-500 text-white text-sm font-medium rounded-xl hover:bg-brand-blue-600 focus:outline-none focus:ring-2 focus:ring-brand-blue-500 focus:ring-offset-2 transition-all duration-150 shadow-sm hover:shadow-md inline-flex items-center gap-2"
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

      {/* Quick Stats */}
      {sessions && (
        <div
          className="grid grid-cols-4 gap-3 mb-8 animate-fade-in-up"
          style={{ animationDelay: "50ms" }}
        >
          <div className="rounded-xl border border-black/[0.06] bg-white/80 px-4 py-3">
            <p className="text-[10px] font-medium uppercase tracking-[0.16em] text-slate-400 mb-0.5">
              Total
            </p>
            <p className="text-xl font-display font-semibold text-slate-900">{sessions.length}</p>
          </div>
          <div className="rounded-xl border border-brand-blue-200/40 bg-brand-blue-50/50 px-4 py-3">
            <p className="text-[10px] font-medium uppercase tracking-[0.16em] text-brand-blue-400 mb-0.5">
              Active
            </p>
            <p className="text-xl font-display font-semibold text-brand-blue-600">
              {sessions.filter((s) => s.status === "connecting" || s.status === "ready").length}
            </p>
          </div>
          <div className="rounded-xl border border-brand-emerald-200/40 bg-brand-emerald-50/50 px-4 py-3">
            <p className="text-[10px] font-medium uppercase tracking-[0.16em] text-brand-emerald-500 mb-0.5">
              Completed
            </p>
            <p className="text-xl font-display font-semibold text-brand-emerald-600">
              {sessions.filter((s) => s.status === "completed").length}
            </p>
          </div>
          <div className="rounded-xl border border-brand-red-200/40 bg-brand-red-50/50 px-4 py-3">
            <p className="text-[10px] font-medium uppercase tracking-[0.16em] text-brand-red-400 mb-0.5">
              Errors
            </p>
            <p className="text-xl font-display font-semibold text-brand-red-500">
              {sessions.filter((s) => s.status === "error").length}
            </p>
          </div>
        </div>
      )}

      {/* Session table */}
      {sessions && sessions.length > 0 ? (
        <div className="animate-fade-in-up" style={{ animationDelay: "100ms" }}>
          <div className="rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm overflow-hidden shadow-[0_1px_3px_rgba(0,0,0,0.04)]">
            {/* Table Header */}
            <div className="px-5 py-3 border-b border-slate-100 bg-slate-50/50">
              <div className="grid grid-cols-[140px_1fr_160px_140px_120px] gap-4 items-center">
                <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-400">
                  Status
                </span>
                <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-400">
                  Specialist
                </span>
                <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-400">
                  Model
                </span>
                <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-400">
                  Created
                </span>
                <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-400 text-right">
                  Actions
                </span>
              </div>
            </div>

            {/* Session Rows */}
            {sessions.map((s) => (
              <Link
                key={s.id}
                to={ROUTES.session(s.id)}
                className={`group block px-5 py-4 border-b border-slate-100/80 last:border-0 transition-colors duration-150 ${
                  s.status === "error" ? "hover:bg-brand-red-50/20" : "hover:bg-brand-blue-50/30"
                }`}
              >
                <div className="grid grid-cols-[140px_1fr_160px_140px_120px] gap-4 items-center">
                  <StatusBadge status={s.status} />
                  <span className="text-sm font-medium text-slate-900">
                    {s.specialist_id ?? <span className="text-slate-400">—</span>}
                  </span>
                  <span className="text-sm text-slate-500 font-mono truncate">
                    {s.model ?? <span className="text-slate-400">—</span>}
                  </span>
                  <span className="text-xs text-slate-400 font-mono">
                    {new Date(s.created_at).toLocaleString()}
                  </span>
                  <span className="text-right">
                    <span className="text-xs font-medium text-brand-blue-600 opacity-0 group-hover:opacity-100 transition-opacity duration-150">
                      Open →
                    </span>
                  </span>
                </div>
              </Link>
            ))}
          </div>
        </div>
      ) : (
        <div
          className="rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm p-12 text-center animate-fade-in-up"
          style={{ animationDelay: "100ms" }}
        >
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
                d="M8.625 12a.375.375 0 11-.75 0 .375.375 0 01.75 0zm0 0H8.25m4.125 0a.375.375 0 11-.75 0 .375.375 0 01.75 0zm0 0H12m4.125 0a.375.375 0 11-.75 0 .375.375 0 01.75 0zm0 0h-.375M21 12c0 4.556-4.03 8.25-9 8.25a9.764 9.764 0 01-2.555-.337A5.972 5.972 0 015.41 20.97a5.969 5.969 0 01-.474-.065 4.48 4.48 0 00.978-2.025c.09-.457-.133-.901-.467-1.226C3.93 16.178 3 14.189 3 12c0-4.556 4.03-8.25 9-8.25s9 3.694 9 8.25z"
              />
            </svg>
          </div>
          <h3 className="text-sm font-medium text-slate-900 mb-1">No sessions</h3>
          <p className="text-sm text-slate-500">
            Create a new session to start coordinating agents
          </p>
        </div>
      )}

      {/* New Session wizard (feat-053) */}
      <NewSessionWizard
        workspaceId={showNewSession ? workspaceId! : null}
        onClose={() => setShowNewSession(false)}
        onCreated={(session) => navigate(ROUTES.session(session.id))}
      />
    </div>
  );
}
