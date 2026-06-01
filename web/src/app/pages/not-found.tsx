import { Link } from "react-router";
import { ROUTES } from "@/lib/routes";

export default function NotFoundPage() {
  return (
    <div className="p-8 text-center">
      <h1 className="text-4xl font-bold">404</h1>
      <p className="text-neutral-500 mt-2">Page not found.</p>
      <Link to={ROUTES.home} className="text-blue-500 hover:underline mt-4 inline-block">
        Go home
      </Link>
    </div>
  );
}
