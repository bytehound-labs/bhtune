import { NavLink, Outlet } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { apiClient } from "../api/client";
import { toApiError } from "../api/errors";

/**
 * The `frontend-shell` health-check badge, relocated from the placeholder `App.tsx` into a
 * permanent spot in the header now that `App.tsx` holds route definitions instead of a
 * single screen.
 */
function HealthBadge() {
  const health = useQuery({
    queryKey: ["health"],
    queryFn: async () => {
      const { data, error, response } = await apiClient.GET("/api/health");
      if (error) throw toApiError(error, response);
      return data;
    },
    refetchInterval: 5000,
  });

  return (
    <div
      className={`rounded-md border px-3 py-1 font-mono text-xs ${
        health.isPending
          ? "border-slate-700 bg-slate-900 text-slate-400"
          : health.isError
            ? "border-red-800 bg-red-950 text-red-300"
            : "border-emerald-800 bg-emerald-950 text-emerald-300"
      }`}
    >
      {health.isPending
        ? "Checking server…"
        : health.isError
          ? `Server unreachable: ${health.error.message}`
          : `Server: ${health.data?.status ?? "unknown"}`}
    </div>
  );
}

const navLinkClass = ({ isActive }: { isActive: boolean }) =>
  `rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
    isActive
      ? "bg-slate-800 text-slate-100"
      : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
  }`;

/** App-wide chrome: header nav + the health badge, wrapping every route via `<Outlet/>`. */
export function AppLayout() {
  return (
    <div className="min-h-screen bg-slate-950 text-slate-100">
      <header className="border-b border-slate-800 bg-slate-900/40">
        <div className="mx-auto flex max-w-5xl items-center justify-between gap-4 px-6 py-4">
          <div className="flex items-center gap-8">
            <span className="text-lg font-semibold tracking-tight">BHTune</span>
            <nav className="flex gap-2">
              <NavLink to="/templates" className={navLinkClass}>
                Templates
              </NavLink>
              <NavLink to="/runs" className={navLinkClass}>
                History
              </NavLink>
            </nav>
          </div>
          <HealthBadge />
        </div>
      </header>
      <main className="mx-auto max-w-5xl px-6 py-8">
        <Outlet />
      </main>
    </div>
  );
}
