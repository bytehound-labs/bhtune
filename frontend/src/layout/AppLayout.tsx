import { NavLink, Outlet, useLocation } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { apiClient } from "../api/client";
import { toApiError } from "../api/errors";

/**
 * Polls the server liveness endpoint and exposes its state as a compact, hoverable indicator.
 * This checks BHTune's HTTP service only; it does not test OPC DA or another process driver.
 */
function HealthIndicator() {
  const health = useQuery({
    queryKey: ["health"],
    queryFn: async () => {
      const { data, error, response } = await apiClient.GET("/api/health");
      if (error) throw toApiError(error, response);
      return data;
    },
    refetchInterval: 5000,
  });

  const status = health.isPending
    ? {
        label: "Connecting to BHTune server",
        detail: "Connecting… checking whether the BHTune server is reachable.",
        dot: "bg-slate-400",
      }
    : health.isError
      ? {
          label: "BHTune server unavailable",
          detail:
            "Connection unavailable — unable to reach the BHTune server. Retrying automatically.",
          dot: "bg-red-400",
        }
      : {
          label: "Connected to BHTune server",
          detail:
            "Connected — the BHTune HTTP service is reachable. This does not test OPC DA connectivity.",
          dot: "bg-emerald-400",
        };

  return (
    <div className="flex items-center gap-3">
      <div
        role="img"
        aria-label={status.label}
        title={status.detail}
        className={`h-2.5 w-2.5 rounded-full shadow-[0_0_0_3px_rgba(15,23,42,0.8)] ${status.dot}`}
      />
      {health.data && (
        <span className="font-mono text-xs text-slate-500">
          v{health.data.version}
        </span>
      )}
    </div>
  );
}

const navLinkClass = ({ isActive }: { isActive: boolean }) =>
  `rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
    isActive
      ? "bg-slate-800 text-slate-100"
      : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
  }`;

/** App-wide chrome: header nav + the health indicator, wrapping every route via `<Outlet/>`. */
export function AppLayout() {
  // `/runs/new` (the "Tune" nav item) is a descendant of `/runs`, so NavLink's default
  // prefix-based active matching would highlight "History" too whenever the user is
  // starting a new tune. Override History's active state to explicitly exclude that one
  // path, so exactly one nav item is ever highlighted at a time.
  const location = useLocation();
  const isHistoryActive =
    location.pathname === "/runs" ||
    (location.pathname.startsWith("/runs/") &&
      !location.pathname.startsWith("/runs/new"));

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100">
      <header className="border-b border-slate-800 bg-slate-900/40">
        <div className="mx-auto flex max-w-5xl items-center justify-between gap-4 px-6 py-4">
          <div className="flex items-center gap-8">
            <span className="text-lg font-semibold tracking-tight">BHTune</span>
            <nav className="flex gap-2">
              <NavLink to="/runs/new" className={navLinkClass}>
                Tune
              </NavLink>
              <NavLink
                to="/runs"
                className={() => navLinkClass({ isActive: isHistoryActive })}
              >
                History
              </NavLink>
              <NavLink to="/templates" className={navLinkClass}>
                Templates
              </NavLink>
            </nav>
          </div>
          <HealthIndicator />
        </div>
      </header>
      <main className="mx-auto max-w-5xl px-6 py-8">
        <Outlet />
      </main>
    </div>
  );
}
