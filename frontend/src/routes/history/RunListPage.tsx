import { useState } from "react";
import { Link } from "react-router";
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
  LoadingState,
  PageHeading,
} from "../../components/ui";

const PROCESS_TYPES = [
  "flow",
  "pressure_line",
  "pressure_vessel",
  "level",
  "temperature_mixing",
  "temperature_heat_exchange",
] as const;
const OUTCOMES = ["running", "completed", "failed", "aborted"] as const;
const DRIVERS = ["opcda", "simulator", "replay"] as const;

const outcomeTone = {
  running: "neutral",
  completed: "success",
  failed: "error",
  aborted: "warning",
} as const;

const PAGE_SIZE = 50;

export function RunListPage({
  capabilities,
}: {
  readonly capabilities: AppCapabilities;
}) {
  const isDemo = capabilities.mode === "demo";
  const [processType, setProcessType] = useState("");
  const [outcome, setOutcome] = useState("");
  const [driver, setDriver] = useState("");
  const [offset, setOffset] = useState(0);

  const filter: RunListFilter = {
    limit: PAGE_SIZE,
    offset,
    ...(processType && {
      process_type: processType as (typeof PROCESS_TYPES)[number],
    }),
    ...(outcome && { outcome: outcome as (typeof OUTCOMES)[number] }),
    ...(driver && { driver: driver as (typeof DRIVERS)[number] }),
  };
  const runs = useRuns(filter, true, isDemo ? "demo" : "full");

  function resetPageAnd(setter: (value: string) => void) {
    return (value: string) => {
      setOffset(0);
      setter(value);
    };
  }

  return (
    <div>
      <PageHeading
        title="History"
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

      {isDemo && (
        <div className="mb-6 rounded-lg border-2 border-amber-700 bg-amber-950/50 px-5 py-4 text-sm text-amber-100">
          <strong>Simulator-only demo.</strong> Every tune and calculated result
          is synthetic and isolated to this browser. Nothing on this page came
          from plant equipment.
          {capabilities.quotas && (
            <>
              {" "}
              History retains up to{" "}
              {capabilities.quotas.retained_runs_per_visitor} runs.
            </>
          )}
        </div>
      )}

      <div className="mb-4 flex flex-wrap gap-3">
        {!isDemo && (
          <select
            value={processType}
            onChange={(e) => resetPageAnd(setProcessType)(e.target.value)}
            className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100"
          >
            <option value="">All process types</option>
            {PROCESS_TYPES.map((p) => (
              <option key={p} value={p}>
                {PROCESS_TYPE_LABELS[p]}
              </option>
            ))}
          </select>
        )}
        {!isDemo && (
          <select
            value={outcome}
            onChange={(e) => resetPageAnd(setOutcome)(e.target.value)}
            className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100"
          >
            <option value="">All outcomes</option>
            {OUTCOMES.map((o) => (
              <option key={o} value={o}>
                {OUTCOME_LABELS[o]}
              </option>
            ))}
          </select>
        )}
        {!isDemo && (
          <select
            value={driver}
            onChange={(e) => resetPageAnd(setDriver)(e.target.value)}
            className="rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100"
          >
            <option value="">All drivers</option>
            {DRIVERS.map((b) => (
              <option key={b} value={b}>
                {DRIVER_LABELS[b]}
              </option>
            ))}
          </select>
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
          <div className="overflow-hidden rounded-lg border border-slate-800">
            <table className="w-full text-left text-sm">
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
                          className="hover:underline"
                        >
                          #{run.id}
                        </Link>
                      </td>
                      <td className="px-4 py-3 font-medium">
                        <Link
                          to={`/runs/${run.id}`}
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
                onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
              >
                Previous
              </Button>
              <Button
                disabled={offset + runs.data.returned >= runs.data.total}
                onClick={() => setOffset(offset + PAGE_SIZE)}
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
