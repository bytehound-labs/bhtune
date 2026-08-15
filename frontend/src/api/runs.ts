import { useQuery } from "@tanstack/react-query";
import { apiClient } from "./client";
import { apiErrorMessage } from "./errors";
import type { components, operations } from "./schema";

export type RunSummaryResponse = components["schemas"]["RunSummaryResponse"];
export type RunDetailResponse = components["schemas"]["RunDetailResponse"];
export type SampleResponse = components["schemas"]["SampleResponse"];
export type ResultResponse = components["schemas"]["ResultResponse"];
export type WriteResponse = components["schemas"]["WriteResponse"];

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

/** `GET /api/runs/{id}` — one run's full detail: config, readings, samples, results, writes. */
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
  });
}
