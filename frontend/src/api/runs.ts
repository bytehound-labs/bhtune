import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiClient } from "./client";
import { apiErrorMessage } from "./errors";
import type { components, operations } from "./schema";

export type RunSummaryResponse = components["schemas"]["RunSummaryResponse"];
export type RunDetailResponse = components["schemas"]["RunDetailResponse"];
export type SampleResponse = components["schemas"]["SampleResponse"];
export type ResultResponse = components["schemas"]["ResultResponse"];
export type WriteResponse = components["schemas"]["WriteResponse"];
export type StartRunRequest = components["schemas"]["StartRunRequest"];

/** Query params accepted by `GET /api/runs` — every field optional (see `RunListQuery`). */
export type RunListFilter = NonNullable<
  operations["list_runs"]["parameters"]["query"]
>;

const runsKey = (filter: RunListFilter) => ["runs", filter] as const;
const runKey = (id: number) => ["runs", id] as const;

/** `GET /api/runs` — a filtered, paginated page of run summaries, newest-started-first. */
export function useRuns(filter: RunListFilter = {}) {
  return useQuery({
    queryKey: runsKey(filter),
    queryFn: async () => {
      const { data, error } = await apiClient.GET("/api/runs", {
        params: { query: filter },
      });
      if (error) throw new Error(apiErrorMessage(error));
      return data;
    },
  });
}

/**
 * `GET /api/runs/{id}` — one run's full detail: config, readings, samples, results, writes.
 * Polls every second while the run is `"running"` (there's no push channel yet — that's
 * `frontend-live-stream`'s SSE endpoint — so this is the interim way to watch a run
 * progress) and stops polling once it reaches a terminal outcome.
 */
export function useRun(id: number) {
  return useQuery({
    queryKey: runKey(id),
    queryFn: async () => {
      const { data, error } = await apiClient.GET("/api/runs/{id}", {
        params: { path: { id } },
      });
      if (error) throw new Error(apiErrorMessage(error));
      return data;
    },
    enabled: Number.isFinite(id),
    refetchInterval: (query) =>
      query.state.data?.outcome === "running" ? 1000 : false,
  });
}

/**
 * `POST /api/runs` — starts a new tune run and returns as soon as `prepare()` succeeds (the
 * run itself keeps going in the background on the server). Seeds the new run's own query
 * cache entry from the `201` response and invalidates the list so `useRuns` picks it up
 * without waiting for its own next poll.
 */
export function useStartRun() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (request: StartRunRequest) => {
      const { data, error } = await apiClient.POST("/api/runs", {
        body: request,
      });
      if (error) throw new Error(apiErrorMessage(error));
      return data;
    },
    onSuccess: (data) => {
      queryClient.setQueryData(runKey(data.id), data);
      void queryClient.invalidateQueries({ queryKey: ["runs"] });
    },
  });
}

/**
 * `POST /api/runs/{id}/cancel` — requests cancellation, exactly as if Ctrl+C had been
 * pressed against an equivalent CLI-driven run. Cancellation is asynchronous (the run's
 * background task still has to observe it and restore the loop), so this only refreshes
 * `useRun`'s query immediately for fast feedback — `useRun`'s own polling picks up the
 * eventual outcome.
 */
export function useCancelRun() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (id: number) => {
      const { error } = await apiClient.POST("/api/runs/{id}/cancel", {
        params: { path: { id } },
      });
      if (error) throw new Error(apiErrorMessage(error));
    },
    onSuccess: (_data, id) => {
      void queryClient.invalidateQueries({ queryKey: runKey(id) });
    },
  });
}
