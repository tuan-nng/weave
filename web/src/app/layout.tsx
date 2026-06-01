import { Link, Outlet } from "react-router";
import { ROUTES } from "@/lib/routes";

export default function AppLayout() {
  return (
    <div className="flex h-screen bg-neutral-50 text-neutral-900">
      <nav className="w-56 border-r border-neutral-200 bg-white p-4 flex flex-col gap-2">
        <Link to={ROUTES.home} className="text-lg font-semibold mb-4">
          Weave
        </Link>
        <Link to={ROUTES.home} className="text-sm hover:underline">
          Home
        </Link>
        <Link to={ROUTES.settings} className="text-sm hover:underline">
          Settings
        </Link>
      </nav>
      <main className="flex-1 overflow-auto">
        <Outlet />
      </main>
    </div>
  );
}
