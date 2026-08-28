import { NavLink, Outlet, useLocation } from "react-router";
import { useQuery } from "@tanstack/react-query";
import { apiClient } from "../api/client";
import { toApiError } from "../api/errors";
import { useTheme } from "../useTheme";

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

  const status = getHealthStatus(health.isPending, health.isError);

  return (
    <div className="flex items-center gap-3 leading-none">
      <span
        aria-hidden="true"
        title={status.detail}
        className={`health-indicator-dot h-2.5 w-2.5 shrink-0 -translate-y-px rounded-full ${status.dot}`}
      />
      <span className="sr-only" role="status">
        {status.label}: {status.detail}
      </span>
      {health.data && (
        <span className="font-mono text-xs leading-none text-slate-500">
          v{health.data.version}
        </span>
      )}
    </div>
  );
}

type HealthStatus = {
  label: string;
  detail: string;
  dot: string;
};

function getHealthStatus(isPending: boolean, isError: boolean): HealthStatus {
  if (isPending) {
    return {
      label: "Connecting to BHTune server",
      detail: "Connecting… checking whether the BHTune server is reachable.",
      dot: "bg-slate-400",
    };
  }

  if (isError) {
    return {
      label: "BHTune server unavailable",
      detail:
        "Connection unavailable — unable to reach the BHTune server. Retrying automatically.",
      dot: "bg-red-400",
    };
  }

  return {
    label: "Connected to BHTune server",
    detail:
      "Connected — the BHTune HTTP service is reachable. This does not test OPC DA connectivity.",
    dot: "bg-emerald-400",
  };
}

function ThemeToggle() {
  const { theme, toggleTheme } = useTheme();
  const nextTheme = theme === "dark" ? "light" : "dark";

  return (
    <button
      type="button"
      onClick={toggleTheme}
      aria-label={`Switch to ${nextTheme} theme`}
      title={`Switch to ${nextTheme} theme`}
      className="inline-flex h-9 w-9 items-center justify-center rounded-md border border-slate-700 bg-slate-900 text-slate-300 transition-colors hover:bg-slate-800 hover:text-slate-100 focus:outline-none focus:ring-2 focus:ring-slate-500"
    >
      {theme === "dark" ? (
        <svg
          aria-hidden="true"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          className="h-4 w-4"
        >
          <circle cx="12" cy="12" r="3.5" />
          <path
            strokeLinecap="round"
            d="M12 2.5v2M12 19.5v2M4.58 4.58l1.42 1.42M18 18l1.42 1.42M2.5 12h2M19.5 12h2M4.58 19.42 6 18M18 6l1.42-1.42"
          />
        </svg>
      ) : (
        <svg
          aria-hidden="true"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          className="h-4 w-4"
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            d="M20.5 15.2A8.5 8.5 0 0 1 8.8 3.5 8.5 8.5 0 1 0 20.5 15.2Z"
          />
        </svg>
      )}
    </button>
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
              <NavLink to="/config" className={navLinkClass}>
                Config
              </NavLink>
            </nav>
          </div>
          <div className="flex items-center gap-4">
            <ThemeToggle />
            <HealthIndicator />
          </div>
        </div>
      </header>
      <main className="mx-auto max-w-5xl px-6 py-8">
        <Outlet />
      </main>
    </div>
  );
}
