import { useState } from "react";
import { useCreateProvider, useDeleteProvider, useProviders } from "../../hooks/use-providers";
import { Modal } from "../../components/modal";
import { ErrorBanner } from "../../components/error-banner";
import { Spinner } from "../../components/spinner";
import type { CreateProviderRequest } from "../../lib/types";

export default function SettingsPage() {
  const { data: providers, isLoading, error } = useProviders();
  const createProvider = useCreateProvider();
  const deleteProvider = useDeleteProvider();

  const [bannerError, setBannerError] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{
    id: string;
    name: string;
  } | null>(null);
  const [form, setForm] = useState<CreateProviderRequest>({
    type: "anthropic",
    name: "",
    base_url: "https://api.anthropic.com",
    api_key: "",
    default_model: "claude-sonnet-4-20250514",
  });

  const handleAdd = (e: React.FormEvent) => {
    e.preventDefault();
    if (!form.name.trim() || !form.api_key.trim()) {
      setBannerError("Name and API Key are required");
      return;
    }

    createProvider.mutate(form, {
      onSuccess: () => {
        setForm({
          type: "anthropic",
          name: "",
          base_url: "https://api.anthropic.com",
          api_key: "",
          default_model: "claude-sonnet-4-20250514",
        });
      },
      onError: (err) => {
        setBannerError(err instanceof Error ? err.message : "Failed to add provider");
      },
    });
  };

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
    return <div className="p-8 text-center text-red-600">Failed to load providers</div>;
  }

  return (
    <div className="max-w-4xl mx-auto px-8 py-8">
      {bannerError && <ErrorBanner message={bannerError} onDismiss={() => setBannerError(null)} />}

      {/* Header */}
      <div className="mb-8">
        <h1 className="text-2xl font-semibold text-neutral-900 tracking-tight">Settings</h1>
        <p className="mt-1 text-sm text-neutral-500">Configure your AI providers</p>
      </div>

      {/* Add provider form */}
      <div className="bg-white rounded-xl border border-neutral-200 p-6 mb-8">
        <h2 className="text-base font-semibold text-neutral-900 mb-5">Add Provider</h2>
        <form onSubmit={handleAdd} className="space-y-4">
          <div className="grid grid-cols-2 gap-4">
            <div>
              <label className="block text-sm font-medium text-neutral-700 mb-1.5">Type</label>
              <input
                type="text"
                value="Anthropic"
                readOnly
                className="w-full h-10 px-3.5 bg-neutral-50 border border-neutral-200 rounded-lg text-sm text-neutral-500 cursor-not-allowed"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-neutral-700 mb-1.5">Name</label>
              <input
                type="text"
                value={form.name}
                onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
                placeholder="e.g. Production"
                className="w-full h-10 px-3.5 bg-white border border-neutral-200 rounded-lg text-sm text-neutral-900 placeholder:text-neutral-400 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                required
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-neutral-700 mb-1.5">Base URL</label>
              <input
                type="text"
                value={form.base_url}
                onChange={(e) => setForm((f) => ({ ...f, base_url: e.target.value }))}
                className="w-full h-10 px-3.5 bg-white border border-neutral-200 rounded-lg text-sm text-neutral-900 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
              />
            </div>
            <div>
              <label className="block text-sm font-medium text-neutral-700 mb-1.5">API Key</label>
              <input
                type="password"
                value={form.api_key}
                onChange={(e) => setForm((f) => ({ ...f, api_key: e.target.value }))}
                placeholder="sk-ant-..."
                className="w-full h-10 px-3.5 bg-white border border-neutral-200 rounded-lg text-sm text-neutral-900 placeholder:text-neutral-400 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                required
              />
            </div>
          </div>
          <div>
            <label className="block text-sm font-medium text-neutral-700 mb-1.5">
              Default Model
            </label>
            <input
              type="text"
              value={form.default_model}
              onChange={(e) => setForm((f) => ({ ...f, default_model: e.target.value }))}
              className="w-full h-10 px-3.5 bg-white border border-neutral-200 rounded-lg text-sm text-neutral-900 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-transparent"
            />
          </div>
          <div className="flex justify-end">
            <button
              type="submit"
              disabled={createProvider.isPending}
              className="h-10 px-4 bg-blue-500 text-white text-sm font-medium rounded-lg hover:bg-blue-600 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 transition-colors disabled:opacity-50"
            >
              Add Provider
            </button>
          </div>
        </form>
      </div>

      {/* Provider list */}
      {providers && providers.length > 0 ? (
        <div className="bg-white rounded-xl border border-neutral-200 overflow-hidden">
          <div className="px-5 py-4 border-b border-neutral-100">
            <h2 className="text-base font-semibold text-neutral-900">Providers</h2>
          </div>
          <table className="w-full">
            <thead>
              <tr className="border-b border-neutral-100">
                <th className="text-left px-5 py-3 text-xs font-medium text-neutral-500 uppercase tracking-wider">
                  Name
                </th>
                <th className="text-left px-5 py-3 text-xs font-medium text-neutral-500 uppercase tracking-wider">
                  Type
                </th>
                <th className="text-left px-5 py-3 text-xs font-medium text-neutral-500 uppercase tracking-wider">
                  Created
                </th>
                <th className="text-right px-5 py-3 text-xs font-medium text-neutral-500 uppercase tracking-wider">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {providers.map((p) => (
                <tr key={p.id} className="border-b border-neutral-100 last:border-0">
                  <td className="px-5 py-3.5 text-sm font-medium text-neutral-900">{p.name}</td>
                  <td className="px-5 py-3.5">
                    <span className="inline-flex items-center px-2.5 py-1 bg-neutral-100 text-neutral-600 text-xs font-medium rounded-md font-mono">
                      {p.type}
                    </span>
                  </td>
                  <td className="px-5 py-3.5 text-sm text-neutral-500 font-mono text-xs">
                    {new Date(p.created_at).toLocaleDateString()}
                  </td>
                  <td className="px-5 py-3.5 text-right">
                    <button
                      onClick={() => setDeleteTarget({ id: p.id, name: p.name })}
                      className="h-8 px-3 text-xs font-medium text-red-600 bg-red-50 border border-red-200 rounded-md hover:bg-red-100 transition-colors"
                    >
                      Delete
                    </button>
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
                d="M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593c.55 0 1.02.398 1.11.94l.213 1.281c.063.374.313.686.645.87.074.04.147.083.22.127.324.196.72.257 1.075.124l1.217-.456a1.125 1.125 0 011.37.49l1.296 2.247a1.125 1.125 0 01-.26 1.431l-1.003.827c-.293.24-.438.613-.431.992a6.759 6.759 0 010 .255c-.007.378.138.75.43.99l1.005.828c.424.35.534.954.26 1.43l-1.298 2.247a1.125 1.125 0 01-1.369.491l-1.217-.456c-.355-.133-.75-.072-1.076.124a6.57 6.57 0 01-.22.128c-.331.183-.581.495-.644.869l-.213 1.28c-.09.543-.56.941-1.11.941h-2.594c-.55 0-1.02-.398-1.11-.94l-.213-1.281c-.062-.374-.312-.686-.644-.87a6.52 6.52 0 01-.22-.127c-.325-.196-.72-.257-1.076-.124l-1.217.456a1.125 1.125 0 01-1.369-.49l-1.297-2.247a1.125 1.125 0 01.26-1.431l1.004-.827c.292-.24.437-.613.43-.992a6.932 6.932 0 010-.255c.007-.378-.138-.75-.43-.99l-1.004-.828a1.125 1.125 0 01-.26-1.43l1.297-2.247a1.125 1.125 0 011.37-.491l1.216.456c.356.133.751.072 1.076-.124.072-.044.146-.087.22-.128.332-.183.582-.495.644-.869l.214-1.281z"
              />
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
              />
            </svg>
          </div>
          <h3 className="text-sm font-medium text-neutral-900 mb-1">No providers</h3>
          <p className="text-sm text-neutral-500">Add an Anthropic provider to get started</p>
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
              Are you sure you want to delete "{deleteTarget?.name}"? Sessions using this provider
              will lose access.
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
            disabled={deleteProvider.isPending}
            className="h-10 px-4 text-sm font-medium text-white bg-red-500 rounded-lg hover:bg-red-600 transition-colors disabled:opacity-50"
          >
            Delete
          </button>
        </div>
      </Modal>
    </div>
  );
}
