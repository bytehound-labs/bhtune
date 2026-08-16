import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiClient } from "./client";
import { toApiError } from "./errors";
import type { components, operations } from "./schema";

export type RunSummaryResponse = components["schemas"]["RunSummaryResponse"];
export type RunDetailResponse = components["schemas"]["RunDetailResponse"];
export type SampleResponse = components["schemas"]["SampleResponse"];
export type ResultResponse = components["schemas"]["ResultResponse"];
export type WriteResponse = components["schemas"]["WriteResponse"];
export type StartRunRequest = components["schemas"]["StartRunRequest"];
export type TuneOutcome = components["schemas"]["TuneOutcome"];

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
      const { data, error, response } = await apiClient.GET("/api/runs", {
        params: { query: filter },
      });
      if (error) throw toApiError(error, response);
      return data;
    },
  });
}

/**
 * `GET /api/runs/{id}` — one run's full detail: config, readings, samples, results, writes.
 *
 * While a run is `"running"`, `useRunStream` (SSE, `frontend-live-stream`) is the live
 * source of truth for per-tick samples — see `RunDetailPage` — so this no longer needs to
 * re-fetch the whole (ever-growing) `samples` array once a second the way it did before
 * that endpoint existed. The 5s poll here is now just a safety net for the terminal
 * `outcome`/`results`/`writes` fields in case the SSE connection never reaches `done` (a
 * proxy that buffers or drops long-lived streams, say); the common case is an immediate,
 * exact-moment refetch triggered by `useRunStream` itself the instant its `done` event
 * arrives, well ahead of this fallback.
 */
export function useRun(id: number) {
  return useQuery({
    queryKey: runKey(id),
    queryFn: async () => {
      const { data, error, response } = await apiClient.GET("/api/runs/{id}", {
        params: { path: { id } },
      });
      if (error) throw toApiError(error, response);
      return data;
    },
    enabled: Number.isFinite(id),
    refetchInterval: (query) =>
      query.state.data?.outcome === "running" ? 5000 : false,
  });
}

/** State kept by {@link useRunStream}. */
export interface RunStreamState {
  /** Every `sample` event received so far, in tick order. The stream always replays every
   * sample from tick 0 on connect (see `bhtune-server`'s `routes::stream` module doc), so
   * this array is a complete, standalone trend the moment the first event arrives — it
   * does not need to be merged with `useRun`'s own `samples`. */
  samples: SampleResponse[];
  /** Set once the terminal `done` event arrives (and the connection has been closed);
   * `null` while still streaming. */
  outcome: TuneOutcome | null;
  /** True after a dropped connection until either a successful reconnect or `done`
   * arrives. `EventSource` retries automatically on its own, so this is informational
   * (e.g. a small "reconnecting…" note) rather than a fatal error state. */
  reconnecting: boolean;
}

const emptyRunStreamState: RunStreamState = {
  samples: [],
  outcome: null,
  reconnecting: false,
};

/**
 * Consumes `GET /api/runs/{id}/stream` over Server-Sent Events to drive a live-updating
 * trend chart while a run is in progress. `enabled` should be `false` once the caller
 * already knows the run is no longer running (see `RunDetailPage`) — there is nothing left
 * to stream for a finished run beyond the instant `done` replay, and `useRun`'s own
 * `samples` is the cheaper source for a run that's already over.
 *
 * Deliberately keeps its own `samples` array rather than appending onto `useRun`'s: the SSE
 * endpoint always replays every sample from tick 0 on every new connection (so it behaves
 * identically whether the page loads mid-run or a dropped connection has to reconnect),
 * which would double-count against whatever `useRun` had already fetched if the two were
 * merged.
 */
export function useRunStream(id: number, enabled: boolean): RunStreamState {
  const queryClient = useQueryClient();
  const [state, setState] = useState<RunStreamState>(emptyRunStreamState);

  useEffect(() => {
    if (!enabled || !Number.isFinite(id)) {
      setState(emptyRunStreamState);
      return;
    }

    setState(emptyRunStreamState);
    const source = new EventSource(`/api/runs/${id}/stream`);

    source.addEventListener("sample", (event) => {
      const sample = JSON.parse(
        (event as MessageEvent<string>).data,
      ) as SampleResponse;
      setState((prev) => ({
        ...prev,
        reconnecting: false,
        samples: [...prev.samples, sample],
      }));
    });

    source.addEventListener("done", (event) => {
      const done = JSON.parse((event as MessageEvent<string>).data) as {
        outcome: TuneOutcome;
      };
      // The server has already ended its response by the time this fires; closing here
      // just pre-empts the browser's own auto-reconnect from racing to reopen a stream
      // with nothing left to say.
      source.close();
      setState((prev) => ({
        ...prev,
        outcome: done.outcome,
        reconnecting: false,
      }));
      void queryClient.invalidateQueries({ queryKey: runKey(id) });
    });

    source.onopen = () => {
      setState((prev) => ({ ...prev, reconnecting: false }));
    };

    source.onerror = () => {
      // A connection the browser has given up on (e.g. the run id turned out not to
      // exist, or the server sent a malformed response) reports `readyState === CLOSED`;
      // anything else is a transient drop `EventSource` is already retrying on its own.
      setState((prev) => ({
        ...prev,
        reconnecting: source.readyState !== EventSource.CLOSED,
      }));
    };

    return () => {
      source.close();
    };
  }, [id, enabled, queryClient]);

  return state;
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
      const { data, error, response } = await apiClient.POST("/api/runs", {
        body: request,
      });
      if (error) throw toApiError(error, response);
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
      const { error, response } = await apiClient.POST(
        "/api/runs/{id}/cancel",
        {
          params: { path: { id } },
        },
      );
      if (error) throw toApiError(error, response);
    },
    onSuccess: (_data, id) => {
      void queryClient.invalidateQueries({ queryKey: runKey(id) });
    },
  });
}

/**
 * `DELETE /api/runs/{id}` — 409 if the run is still active (cancel it first). Removes the
 * run and its samples/results/write-back audit rows in one cascade (see `db-schema`'s
 * `ON DELETE CASCADE`); there is no undo.
 */
export function useDeleteRun() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: async (id: number) => {
      const { error, response } = await apiClient.DELETE("/api/runs/{id}", {
        params: { path: { id } },
      });
      if (error) throw toApiError(error, response);
    },
    onSuccess: (_data, id) => {
      queryClient.removeQueries({ queryKey: runKey(id) });
      void queryClient.invalidateQueries({ queryKey: ["runs"] });
    },
  });
}

/**
 * The URL for `GET /api/runs/{id}/export` — a plain link/anchor `href`, not a query hook:
 * the browser's native download handling (triggered by the response's
 * `Content-Disposition: attachment` header) is simpler and more robust than fetching the
 * bytes in JS just to hand them back to the browser via a manufactured object URL.
 */
export function runExportUrl(id: number, format: "csv" | "json"): string {
  return `/api/runs/${id}/export?format=${format}`;
}
