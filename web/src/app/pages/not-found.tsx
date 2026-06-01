import { Link } from "react-router";
import { ROUTES } from "@/lib/routes";

export default function NotFoundPage() {
  return (
    <div className="flex items-center justify-center h-full">
      <div className="text-center animate-fade-in">
        <h1 className="font-display text-6xl font-bold tracking-tight text-slate-900">404</h1>
        <p className="text-slate-500 mt-3 text-sm">Page not found.</p>
        <Link
          to={ROUTES.home}
          className="inline-flex items-center gap-1.5 text-sm font-medium text-brand-blue-600 hover:text-brand-blue-700 mt-6 transition-colors"
        >
          <svg
            className="w-4 h-4"
            fill="none"
            viewBox="0 0 24 24"
            stroke="currentColor"
            strokeWidth={2}
          >
            <path strokeLinecap="round" strokeLinejoin="round" d="M15 19l-7-7 7-7" />
          </svg>
          Go home
        </Link>
      </div>
    </div>
  );
}
