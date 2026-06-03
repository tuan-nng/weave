// CodebasePage — `/workspaces/:wid/codebases/:cid`.
// Composite detail: codebase row + git status (branch, dirty files,
// recent commits). When `git_error` is set, we surface a banner
// with the message instead of the status block. A delete button in
// the header removes the codebase from the workspace.

import { useCodebase, useDeleteCodebase } from "../../hooks/use-codebase";
import { ErrorBanner } from "../../components/error-banner";
import { Spinner } from "../../components/spinner";
import { Link, useNavigate, useParams } from "react-router";
import { ROUTES } from "../../lib/routes";

export default function CodebasePage() {
  const { wid, cid } = useParams<{ wid: string; cid: string }>();
  const workspaceId = wid ?? "";
  const codebaseId = cid ?? "";
  const navigate = useNavigate();

  const { data: detail, isLoading, isError, error } = useCodebase(workspaceId, codebaseId);
  const deleteMutation = useDeleteCodebase(workspaceId);

  if (!workspaceId || !codebaseId) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-slate-500">Missing codebase id.</p>
      </div>
    );
  }

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-full">
        <Spinner />
      </div>
    );
  }

  if (isError) {
    return (
      <div className="p-8 max-w-4xl mx-auto">
        <ErrorBanner
          message={error?.message ?? "Failed to load codebase"}
          onDismiss={() => window.location.reload()}
        />
      </div>
    );
  }

  if (!detail) {
    return (
      <div className="flex items-center justify-center h-full">
        <p className="text-slate-500">Codebase not found.</p>
      </div>
    );
  }

  const { codebase, git_status, git_error } = detail;

  return (
    <div className="flex flex-col h-full bg-[#fafafa]">
      <header className="flex-shrink-0 h-14 flex items-center justify-between px-5 bg-white/80 backdrop-blur-sm border-b border-slate-200/80">
        <div className="flex items-center gap-2.5">
          <Link
            to={ROUTES.codebases}
            className="p-1.5 rounded-lg text-slate-400 hover:text-slate-600 hover:bg-slate-100/60 transition-colors"
            aria-label="Back to codebases"
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
              <polyline points="15 18 9 12 15 6" />
            </svg>
          </Link>
          <h1 className="text-sm font-semibold text-slate-900">
            {codebase.label ?? codebase.path}
          </h1>
          <span className="text-[10px] font-mono text-slate-400 bg-slate-50 border border-slate-200/60 rounded-md px-1.5 py-0.5">
            {codebaseId.slice(0, 8)}
          </span>
        </div>
        <button
          type="button"
          onClick={() => {
            if (window.confirm(`Delete codebase at ${codebase.path}?`)) {
              deleteMutation.mutate(codebaseId, {
                onSuccess: () => navigate(ROUTES.codebases),
              });
            }
          }}
          disabled={deleteMutation.isPending}
          className="h-9 px-3 text-sm font-medium text-red-600 bg-white border border-red-200 rounded-lg hover:bg-red-50 transition-colors disabled:opacity-50"
        >
          {deleteMutation.isPending ? "Deleting…" : "Delete"}
        </button>
      </header>

      <main className="flex-1 min-h-0 overflow-y-auto p-8 max-w-4xl mx-auto w-full">
        {/* Codebase identity */}
        <section className="bg-white rounded-2xl border border-slate-200/60 shadow-sm p-6 mb-6">
          <h2 className="text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-3">
            Codebase
          </h2>
          <dl className="grid grid-cols-[120px_1fr] gap-y-3 gap-x-4 text-sm">
            <dt className="text-slate-500">Path</dt>
            <dd className="font-mono text-slate-900 break-all">{codebase.path}</dd>
            <dt className="text-slate-500">Label</dt>
            <dd className="text-slate-900">{codebase.label ?? "—"}</dd>
            <dt className="text-slate-500">Branch</dt>
            <dd className="font-mono text-slate-900">{codebase.branch ?? "—"}</dd>
            <dt className="text-slate-500">Workspace</dt>
            <dd className="font-mono text-slate-700">{workspaceId.slice(0, 8)}</dd>
            <dt className="text-slate-500">Created</dt>
            <dd className="text-slate-700">{new Date(codebase.created_at).toLocaleString()}</dd>
          </dl>
        </section>

        {/* Git status — graceful degrade when path is not a repo */}
        {git_error && (
          <section className="mb-6">
            <ErrorBanner message={git_error} onDismiss={() => {}} />
          </section>
        )}

        {git_status && (
          <>
            <section className="bg-white rounded-2xl border border-slate-200/60 shadow-sm p-6 mb-6">
              <h2 className="text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-3">
                Git status
              </h2>
              <dl className="grid grid-cols-[120px_1fr] gap-y-3 gap-x-4 text-sm">
                <dt className="text-slate-500">Branch</dt>
                <dd className="font-mono text-slate-900">{git_status.branch || "—"}</dd>
                <dt className="text-slate-500">Dirty files</dt>
                <dd>
                  {git_status.dirty_files.length === 0 ? (
                    <span className="text-slate-500">clean</span>
                  ) : (
                    <ul className="font-mono text-slate-900 space-y-0.5">
                      {git_status.dirty_files.map((p) => (
                        <li key={p}>{p}</li>
                      ))}
                    </ul>
                  )}
                </dd>
              </dl>
            </section>

            <section className="bg-white rounded-2xl border border-slate-200/60 shadow-sm p-6">
              <h2 className="text-[11px] font-semibold uppercase tracking-[0.14em] text-slate-400 mb-3">
                Recent commits
              </h2>
              {git_status.recent_commits.length === 0 ? (
                <p className="text-sm text-slate-500">No commits yet.</p>
              ) : (
                <ol className="space-y-2">
                  {git_status.recent_commits.map((c) => (
                    <li key={c.hash} className="flex items-baseline gap-3 text-sm">
                      <code className="font-mono text-[11px] text-slate-500 bg-slate-50 border border-slate-200/60 rounded px-1.5 py-0.5">
                        {c.hash.slice(0, 7)}
                      </code>
                      <span className="text-slate-900 truncate">{c.message}</span>
                    </li>
                  ))}
                </ol>
              )}
            </section>
          </>
        )}
      </main>
    </div>
  );
}
