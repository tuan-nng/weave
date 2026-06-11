import { useState } from "react";
import { useDeleteProvider, useProviders } from "../../hooks/use-providers";
import { Modal } from "../../components/modal";
import { AddProviderModal } from "../../components/add-provider-modal";
import { ErrorBanner } from "../../components/error-banner";
import { Spinner } from "../../components/spinner";

export default function SettingsPage() {
  const { data: providers, isLoading, error } = useProviders();
  const deleteProvider = useDeleteProvider();

  const [bannerError, setBannerError] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{
    id: string;
    name: string;
  } | null>(null);
  // Feat-052: the "Add Provider" form moved to AddProviderModal. The
  // page now hosts a `+ Add Provider` button + the modal; the form
  // state (kind picker, http/cli arms, args/env editors) lives in the
  // modal component.
  const [addOpen, setAddOpen] = useState(false);

  const handleDelete = () => {
    if (!deleteTarget) return;
    deleteProvider.mutate(deleteTarget.id, {
      onSuccess: () => setDeleteTarget(null),
      onError: (err) => {
        setBannerError(err instanceof Error ? err.message : "Failed to delete provider");
        setDeleteTarget(null);
      },
    });
  };

  if (isLoading) return <Spinner />;

  if (error) {
    return <div className="p-8 text-center text-brand-red-600">Failed to load providers</div>;
  }

  return (
    <div className="max-w-4xl mx-auto px-8 py-8 lg:px-12 lg:py-10">
      {bannerError && <ErrorBanner message={bannerError} onDismiss={() => setBannerError(null)} />}

      {/* Header */}
      <div className="mb-8 animate-fade-in">
        <h1 className="font-display text-2xl font-semibold tracking-tight text-slate-900">
          Settings
        </h1>
        <p className="mt-1.5 text-sm text-slate-500">Configure your AI providers and preferences</p>
      </div>

      {/* Add provider button — opens the kind-aware AddProviderModal */}
      <div className="mb-8 animate-fade-in-up" style={{ animationDelay: "50ms" }}>
        <button
          type="button"
          onClick={() => setAddOpen(true)}
          className="h-10 px-5 bg-brand-blue-500 text-white text-sm font-medium rounded-xl hover:bg-brand-blue-600 focus:outline-none focus:ring-2 focus:ring-brand-blue-500 focus:ring-offset-2 transition-all duration-150 shadow-sm hover:shadow-md inline-flex items-center gap-2"
        >
          <svg
            className="w-4 h-4"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path strokeLinecap="round" strokeLinejoin="round" d="M12 6v6m0 0v6m0-6h6m-6 0H6" />
          </svg>
          Add Provider
        </button>
      </div>

      {/* Provider list */}
      {providers && providers.length > 0 ? (
        <div className="animate-fade-in-up" style={{ animationDelay: "100ms" }}>
          <h2 className="text-base font-semibold text-slate-900 mb-4">Providers</h2>
          <div className="rounded-2xl border border-black/[0.06] bg-white/80 backdrop-blur-sm overflow-hidden shadow-[0_1px_3px_rgba(0,0,0,0.04)]">
            {/* Table Header */}
            <div className="px-5 py-3 border-b border-slate-100 bg-slate-50/50">
              <div className="grid grid-cols-[1fr_100px_140px_100px] gap-4 items-center">
                <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-400">
                  Name
                </span>
                <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-400">
                  Type
                </span>
                <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-400">
                  Created
                </span>
                <span className="text-[10px] font-semibold uppercase tracking-[0.16em] text-slate-400 text-right">
                  Actions
                </span>
              </div>
            </div>

            {/* Provider Rows */}
            {providers.map((p) => (
              <div
                key={p.id}
                className="group px-5 py-4 hover:bg-slate-50/50 transition-colors duration-150"
              >
                <div className="grid grid-cols-[1fr_100px_140px_100px] gap-4 items-center">
                  <div className="flex items-center gap-3">
                    <div className="w-8 h-8 rounded-lg bg-gradient-to-br from-brand-amber-50 to-brand-amber-100 border border-brand-amber-200/60 flex items-center justify-center flex-shrink-0">
                      <svg
                        className="w-4 h-4 text-brand-amber-600"
                        fill="none"
                        viewBox="0 0 24 24"
                        stroke="currentColor"
                        strokeWidth={1.8}
                      >
                        <path
                          strokeLinecap="round"
                          strokeLinejoin="round"
                          d="M13 10V3L4 14h7v7l9-11h-7z"
                        />
                      </svg>
                    </div>
                    <span className="text-sm font-medium text-slate-900">{p.name}</span>
                    <span
                      className={
                        "inline-flex items-center px-2 py-0.5 rounded-md text-[10px] font-mono font-semibold tracking-wide border w-fit uppercase " +
                        (p.kind === "cli"
                          ? "bg-brand-orchid-50 text-brand-orchid-600 border-brand-orchid-200/60"
                          : "bg-slate-100 text-slate-600 border-slate-200/60")
                      }
                    >
                      {p.kind}
                    </span>
                  </div>
                  <span className="inline-flex items-center px-2 py-0.5 rounded-md text-[10px] font-mono font-semibold tracking-wide bg-slate-100 text-slate-600 border border-slate-200/60 w-fit uppercase">
                    {p.type}
                  </span>
                  <span className="text-xs text-slate-400 font-mono">
                    {new Date(p.created_at).toLocaleDateString()}
                  </span>
                  <div className="text-right">
                    <button
                      onClick={() => setDeleteTarget({ id: p.id, name: p.name })}
                      className="h-7 px-2.5 text-xs font-medium text-brand-red-600 bg-brand-red-50 border border-brand-red-200/60 rounded-lg hover:bg-brand-red-100 transition-all duration-150 opacity-0 group-hover:opacity-100"
                    >
                      Delete
                    </button>
                  </div>
                </div>
              </div>
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
                d="M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593c.55 0 1.02.398 1.11.94l.213 1.281c.063.374.313.686.645.87.074.04.147.083.22.127.324.196.72.257 1.075.124l1.217-.456a1.125 1.125 0 011.37.49l1.296 2.247a1.125 1.125 0 01-.26 1.431l-1.003.827c-.293.24-.438.613-.431.992a6.759 6.759 0 010 .255c-.007.378.138.75.43.99l1.005.828c.424.35.534.954.26 1.43l-1.298 2.247a1.125 1.125 0 01-1.369.491l-1.217-.456c-.355-.133-.75-.072-1.076.124a6.57 6.57 0 01-.22.128c-.331.183-.581.495-.644.869l-.213 1.28c-.09.543-.56.941-1.11.941h-2.594c-.55 0-1.02-.398-1.11-.94l-.213-1.281c-.062-.374-.312-.686-.644-.87a6.52 6.52 0 01-.22-.127c-.325-.196-.72-.257-1.076-.124l-1.217.456a1.125 1.125 0 01-1.369-.49l-1.297-2.247a1.125 1.125 0 01.26-1.431l1.004-.827c.292-.24.437-.613.43-.992a6.932 6.932 0 010-.255c.007-.378-.138-.75-.43-.99l-1.004-.828a1.125 1.125 0 01-.26-1.43l1.297-2.247a1.125 1.125 0 011.37-.491l1.216.456c.356.133.751.072 1.076-.124.072-.044.146-.087.22-.128.332-.183.582-.495.644-.869l.214-1.281z"
              />
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
              />
            </svg>
          </div>
          <h3 className="text-sm font-medium text-slate-900 mb-1">No providers</h3>
          <p className="text-sm text-slate-500">Add a provider to get started</p>
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
              Are you sure you want to delete "{deleteTarget?.name}"? Sessions using this provider
              will lose access.
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
            disabled={deleteProvider.isPending}
            className="h-10 px-4 text-sm font-medium text-white bg-brand-red-500 rounded-xl hover:bg-brand-red-600 transition-all duration-150 disabled:opacity-50"
          >
            Delete
          </button>
        </div>
      </Modal>

      {/* Add provider modal — kind-aware form (HTTP or CLI) */}
      <AddProviderModal open={addOpen} onClose={() => setAddOpen(false)} />
    </div>
  );
}
