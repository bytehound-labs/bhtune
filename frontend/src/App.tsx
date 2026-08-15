import { useQuery } from "@tanstack/react-query";
import { apiClient } from "./api/client";

// This is the frontend shell: a minimal placeholder that proves the whole
// pipeline (Vite dev proxy / embedded-SPA production build → React →
// TanStack Query → the openapi-fetch client generated from openapi.json →
// bhtune-server's real HTTP API) works end to end. The real screens
// (Connection, Tag mapping, Test parameters, Results, History, Template
// editor, Simulator) land in the `frontend-screens` phase.
function App() {
  const health = useQuery({
    queryKey: ["health"],
    queryFn: async () => {
      const { data, error } = await apiClient.GET("/api/health");
      if (error) throw error;
      return data;
    },
    refetchInterval: 5000,
  });

  return (
    <div className="flex min-h-screen flex-col items-center justify-center gap-4 bg-slate-950 p-8 text-slate-100">
      <h1 className="text-3xl font-semibold tracking-tight">BHTune</h1>
      <p className="text-slate-400">
        FOSS PID auto-tuner — web GUI shell (frontend-shell phase).
      </p>
      <div
        className={`rounded-md border px-4 py-2 font-mono text-sm ${
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
            ? `Server unreachable: ${String(health.error)}`
            : `Server status: ${health.data?.status ?? "unknown"}`}
      </div>
    </div>
  );
}

export default App;
