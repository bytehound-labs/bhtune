import { useEffect, useId } from "react";
import { Link, useLocation, useSearchParams } from "react-router";
import { useRuns } from "../../api/runs";
import type { RunListFilter } from "../../api/runs";
import { userFacingErrorMessage } from "../../api/errors";
import {
  DRIVER_LABELS,
  OUTCOME_LABELS,
  PROCESS_TYPE_LABELS,
} from "../../lib/enumLabels";
import type { AppCapabilities } from "../../api/capabilities";
import {
  Badge,
  Button,
  EmptyState,
  ErrorBanner,
  InlineStatus,
  LoadingState,
  PageHeading,
} from "../../components/ui";
import {
  HISTORY_DRIVERS,
  HISTORY_OUTCOMES,
  HISTORY_PAGE_SIZE,
  HISTORY_PROCESS_TYPES,
  historyUrlSearchParams,
  parseHistoryUrlState,
} from "./historyUrlState";

const outcomeTone = {
  running: "neutral",
  completed: "success",
  failed: "error",
  aborted: "warning",
} as const;

function storedHistoryNotice(state: unknown, search: string): string | null {
  if (
    !state ||
    typeof state !== "object" ||
    !("historyNotice" in state) ||
    !state.historyNotice ||
    typeof state.historyNotice !== "object" ||
    !("search" in state.historyNotice) ||
    !("message" in state.historyNotice)
  ) {
    return null;
  }

  return state.historyNotice.search === search &&
    typeof state.historyNotice.message === "string"
    ? state.historyNotice.message
    : null;
}

export function RunListPage({
  capabilities,
}: {
  readonly capabilities: AppCapabilities;
}) {
  const isDemo = capabilities.mode === "demo";
  const location = useLocation();
  const [searchParams, setSearchParams] = useSearchParams();
  const rawSearch = searchParams.toString();
  const parsedUrl = parseHistoryUrlState(
    new URLSearchParams(rawSearch),
    isDemo,
  );
  const { processType, outcome, driver, offset } = parsedUrl.state;
  const invalidParameterKey = parsedUrl.invalidParameters.join(",");
  const unavailableInDemo =
    isDemo &&
    parsedUrl.invalidParameters.some((parameter) => parameter !== "offset");
  const normalizedSearch = historyUrlSearchParams(
    searchParams,
    parsedUrl.state,
    isDemo,
  ).toString();
  const invalidUrlMessage = invalidParameterKey
    ? unavailableInDemo
      ? "History filters are unavailable in Demo mode. Unsupported URL values were reset."
      : "Invalid history filter or page URL values were reset."
    : null;
  const urlNoticeMessage =
    invalidUrlMessage ?? storedHistoryNotice(location.state, rawSearch);
  const processTypeFilterId = useId();
  const outcomeFilterId = useId();
  const driverFilterId = useId();

  const filter: RunListFilter = {
    limit: HISTORY_PAGE_SIZE,
    offset,
    ...(processType && {
      process_type: processType,
    }),
    ...(outcome && { outcome }),
    ...(driver && { driver }),
  };
  const runs = useRuns(filter, true, isDemo ? "demo" : "full");

  useEffect(() => {
    if (rawSearch === normalizedSearch) return;

    setSearchParams(new URLSearchParams(normalizedSearch), {
      replace: true,
      state: invalidUrlMessage
        ? {
            historyNotice: {
              search: normalizedSearch,
              message: invalidUrlMessage,
            },
          }
        : null,
    });
  }, [invalidUrlMessage, normalizedSearch, rawSearch, setSearchParams]);

  const lastAvailableOffset = runs.isSuccess
    ? Math.max(0, Math.ceil(runs.data.total / HISTORY_PAGE_SIZE) - 1) *
      HISTORY_PAGE_SIZE
    : null;
  const clampedSearch =
    lastAvailableOffset !== null && offset > lastAvailableOffset
      ? historyUrlSearchParams(
          searchParams,
          { ...parsedUrl.state, offset: lastAvailableOffset },
          isDemo,
        ).toString()
      : null;

  useEffect(() => {
    if (clampedSearch === null) return;
    setSearchParams(new URLSearchParams(clampedSearch), {
      replace: true,
      state: {
        historyNotice: {
          search: clampedSearch,
          message:
            "That history page is no longer available. Showing the last available page.",
        },
      },
    });
  }, [clampedSearch, setSearchParams]);

  function updateHistory(nextState: typeof parsedUrl.state) {
    setSearchParams(historyUrlSearchParams(searchParams, nextState, isDemo), {
      state: null,
    });
  }

  const historyHref = normalizedSearch ? `/runs?${normalizedSearch}` : "/runs";

  return (
    <div>
      <PageHeading
        title="History"
        documentationId="history.page"
        description={
          isDemo
            ? "Review synthetic tunes created in this browser session."
            : "Review completed tunes and monitor active tunes."
        }
        actions={
          <Link to="/runs/new">
            <Button variant="primary">New tune</Button>
          </Link>
        }
      />

      {urlNoticeMessage && (
        <InlineStatus message={urlNoticeMessage} tone="warning" />
      )}

      <div className="mb-4 flex flex-wrap gap-3">
        {!isDemo && (
          <>
            <label className="sr-only" htmlFor={processTypeFilterId}>
              Filter by process type
            </label>
            <select
              id={processTypeFilterId}
              value={processType}
              onChange={(e) =>
                updateHistory({
                  ...parsedUrl.state,
                  processType: e.target
                    .value as typeof parsedUrl.state.processType,
                  offset: 0,
                })
              }
              className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100"
            >
              <option value="">All process types</option>
              {HISTORY_PROCESS_TYPES.map((p) => (
                <option key={p} value={p}>
                  {PROCESS_TYPE_LABELS[p]}
                </option>
              ))}
            </select>
          </>
        )}
        {!isDemo && (
          <>
            <label className="sr-only" htmlFor={outcomeFilterId}>
              Filter by outcome
            </label>
            <select
              id={outcomeFilterId}
              value={outcome}
              onChange={(e) =>
                updateHistory({
                  ...parsedUrl.state,
                  outcome: e.target.value as typeof parsedUrl.state.outcome,
                  offset: 0,
                })
              }
              className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100"
            >
              <option value="">All outcomes</option>
              {HISTORY_OUTCOMES.map((o) => (
                <option key={o} value={o}>
                  {OUTCOME_LABELS[o]}
                </option>
              ))}
            </select>
          </>
        )}
        {!isDemo && (
          <>
            <label className="sr-only" htmlFor={driverFilterId}>
              Filter by driver
            </label>
            <select
              id={driverFilterId}
              value={driver}
              onChange={(e) =>
                updateHistory({
                  ...parsedUrl.state,
                  driver: e.target.value as typeof parsedUrl.state.driver,
                  offset: 0,
                })
              }
              className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100"
            >
              <option value="">All drivers</option>
              {HISTORY_DRIVERS.map((b) => (
                <option key={b} value={b}>
                  {DRIVER_LABELS[b]}
                </option>
              ))}
            </select>
          </>
        )}
      </div>

      {runs.isPending && <LoadingState message="Loading runs…" />}
      {runs.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            runs.error,
            "Unable to load tune history.",
            isDemo,
          )}
        />
      )}
      {runs.isSuccess && runs.data.runs.length === 0 && (
        <EmptyState message="No tunes match this filter." />
      )}

      {runs.isSuccess && runs.data.runs.length > 0 && (
        <>
          <div className="overflow-x-auto rounded-lg border border-slate-800">
            <table className="w-full min-w-[48rem] text-left text-sm">
              <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
                <tr>
                  <th className="px-4 py-2 font-medium">ID</th>
                  <th className="px-4 py-2 font-medium">
                    {isDemo ? "Tune" : "Tag name"}
                  </th>
                  <th className="px-4 py-2 font-medium">Process type</th>
                  <th className="px-4 py-2 font-medium">Outcome</th>
                  {!isDemo && <th className="px-4 py-2 font-medium">Driver</th>}
                  <th className="px-4 py-2 font-medium">Started</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-800">
                {runs.data.runs.map((run) => {
                  return (
                    <tr key={run.id} className="hover:bg-slate-900/30">
                      <td className="px-4 py-3 font-mono text-slate-400">
                        <Link
                          to={`/runs/${run.id}`}
                          state={{ historyHref }}
                          className="hover:underline"
                        >
                          #{run.id}
                        </Link>
                      </td>
                      <td className="px-4 py-3 font-medium">
                        <Link
                          to={`/runs/${run.id}`}
                          state={{ historyHref }}
                          className="hover:underline"
                        >
                          {isDemo ? "Simulator demo" : run.tag_name}
                        </Link>
                      </td>
                      <td className="px-4 py-3 text-slate-400">
                        {PROCESS_TYPE_LABELS[run.process_type]}
                      </td>
                      <td className="px-4 py-3">
                        <Badge tone={outcomeTone[run.outcome]}>
                          {OUTCOME_LABELS[run.outcome]}
                        </Badge>
                      </td>
                      {!isDemo && (
                        <td className="px-4 py-3 text-slate-400">
                          {DRIVER_LABELS[run.driver]}
                        </td>
                      )}
                      <td className="px-4 py-3 text-slate-400">
                        {new Date(run.started_at).toLocaleString()}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>

          <div className="mt-4 flex items-center justify-between text-sm text-slate-400">
            <span>
              Showing {offset + 1}–{offset + runs.data.returned} of{" "}
              {runs.data.total}
            </span>
            <div className="flex gap-2">
              <Button
                disabled={offset === 0}
                onClick={() =>
                  updateHistory({
                    ...parsedUrl.state,
                    offset: Math.max(0, offset - HISTORY_PAGE_SIZE),
                  })
                }
              >
                Previous
              </Button>
              <Button
                disabled={offset + runs.data.returned >= runs.data.total}
                onClick={() =>
                  updateHistory({
                    ...parsedUrl.state,
                    offset: offset + HISTORY_PAGE_SIZE,
                  })
                }
              >
                Next
              </Button>
            </div>
          </div>
        </>
      )}
    </div>
  );
}
