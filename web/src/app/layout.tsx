import { Link, Outlet, useLocation } from "react-router";
import { ROUTES } from "@/lib/routes";

const NAV_ITEMS = [
  {
    label: "Home",
    href: ROUTES.home,
    icon: (
      <svg
        className="w-[18px] h-[18px]"
        fill="none"
        viewBox="0 0 24 24"
        stroke="currentColor"
        strokeWidth={1.8}
      >
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          d="M3 12l2-2m0 0l7-7 7 7M5 10v10a1 1 0 001 1h3m10-11l2 2m-2-2v10a1 1 0 01-1 1h-3m-6 0a1 1 0 001-1v-4a1 1 0 011-1h2a1 1 0 011 1v4a1 1 0 001 1m-6 0h6"
        />
      </svg>
    ),
  },
  {
    label: "Sessions",
    href: ROUTES.sessions,
    icon: (
      <svg
        className="w-[18px] h-[18px]"
        fill="none"
        viewBox="0 0 24 24"
        stroke="currentColor"
        strokeWidth={1.8}
      >
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z"
        />
      </svg>
    ),
  },
  {
    label: "Kanban",
    href: ROUTES.home,
    icon: (
      <svg
        className="w-[18px] h-[18px]"
        fill="none"
        viewBox="0 0 24 24"
        stroke="currentColor"
        strokeWidth={1.8}
      >
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          d="M9 17V7m0 10a2 2 0 01-2 2H5a2 2 0 01-2-2V7a2 2 0 012-2h2a2 2 0 012 2m0 10a2 2 0 002 2h2a2 2 0 002-2M9 7a2 2 0 012-2h2a2 2 0 012 2m0 10V7m0 10a2 2 0 002 2h2a2 2 0 002-2V7a2 2 0 00-2-2h-2a2 2 0 00-2 2"
        />
      </svg>
    ),
  },
];

const SECONDARY_NAV = [
  {
    label: "Settings",
    href: ROUTES.settings,
    icon: (
      <svg
        className="w-[18px] h-[18px]"
        fill="none"
        viewBox="0 0 24 24"
        stroke="currentColor"
        strokeWidth={1.8}
      >
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"
        />
        <path strokeLinecap="round" strokeLinejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
      </svg>
    ),
  },
];

export default function AppLayout() {
  const location = useLocation();

  const isActive = (href: string) => {
    if (href === "/") return location.pathname === "/";
    return location.pathname.startsWith(href);
  };

  return (
    <div className="flex h-screen bg-[#fafafa] text-slate-900 antialiased overflow-hidden">
      <aside className="w-60 flex-shrink-0 flex flex-col bg-white border-r border-slate-200/80">
        {/* Brand */}
        <div className="px-5 h-14 flex items-center gap-2.5 border-b border-slate-100">
          <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-brand-blue-500 to-brand-blue-700 flex items-center justify-center">
            <svg
              className="w-4 h-4 text-white"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path strokeLinecap="round" strokeLinejoin="round" d="M13 10V3L4 14h7v7l9-11h-7z" />
            </svg>
          </div>
          <span className="font-display text-[15px] font-semibold tracking-tight text-slate-900">
            Weave
          </span>
        </div>

        {/* Navigation */}
        <nav className="flex-1 px-3 py-3 space-y-0.5 overflow-y-auto">
          {NAV_ITEMS.map((item) => {
            const active = isActive(item.href);
            return (
              <Link
                key={item.label}
                to={item.href}
                className={`group flex items-center gap-3 px-3 h-10 rounded-xl text-sm font-medium transition-all duration-150 relative ${
                  active
                    ? "bg-brand-blue-50 text-brand-blue-700"
                    : "text-slate-600 hover:bg-slate-50 hover:text-slate-900"
                }`}
              >
                {active && (
                  <div className="absolute left-0 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-r bg-brand-blue-500" />
                )}
                <span
                  className={
                    active ? "text-brand-blue-500" : "text-slate-400 group-hover:text-slate-500"
                  }
                >
                  {item.icon}
                </span>
                <span>{item.label}</span>
              </Link>
            );
          })}

          <div className="!my-3 border-t border-slate-100" />

          {SECONDARY_NAV.map((item) => {
            const active = isActive(item.href);
            return (
              <Link
                key={item.label}
                to={item.href}
                className={`group flex items-center gap-3 px-3 h-10 rounded-xl text-sm font-medium transition-all duration-150 relative ${
                  active
                    ? "bg-brand-blue-50 text-brand-blue-700"
                    : "text-slate-600 hover:bg-slate-50 hover:text-slate-900"
                }`}
              >
                {active && (
                  <div className="absolute left-0 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-r bg-brand-blue-500" />
                )}
                <span
                  className={
                    active ? "text-brand-blue-500" : "text-slate-400 group-hover:text-slate-500"
                  }
                >
                  {item.icon}
                </span>
                <span>{item.label}</span>
              </Link>
            );
          })}
        </nav>

        {/* Sidebar Footer */}
        <div className="px-3 py-3 border-t border-slate-100">
          <div className="flex items-center gap-3 px-3 py-2 rounded-xl bg-slate-50/80">
            <div className="w-7 h-7 rounded-full bg-gradient-to-br from-brand-blue-400 to-brand-orchid-500 flex items-center justify-center text-white text-xs font-semibold">
              W
            </div>
            <div className="flex-1 min-w-0">
              <p className="text-xs font-medium text-slate-900 truncate">Default Workspace</p>
              <p className="text-[10px] text-slate-400">Active</p>
            </div>
          </div>
        </div>
      </aside>

      <main className="flex-1 min-w-0 overflow-y-auto">
        <Outlet />
      </main>
    </div>
  );
}
