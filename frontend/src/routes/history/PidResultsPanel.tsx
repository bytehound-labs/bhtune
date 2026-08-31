import type { RunDetailResponse, ResponseLevel } from "../../api/runs";
import { RESPONSE_LEVEL_LABELS } from "../../lib/enumLabels";
import { Button } from "../../components/ui";
import {
  formatNumber,
  type RunResult,
  type WriteEligibility,
} from "./runDetailHelpers";

interface PidResultsPanelProps {
  readonly run: RunDetailResponse;
  readonly eligibility: WriteEligibility;
  readonly writePending: boolean;
  readonly writingResponseLevel?: ResponseLevel;
  readonly promoted?: boolean;
  readonly onWrite: (result: RunResult) => void;
}

export function PidResultsPanel({
  run,
  eligibility,
  writePending,
  writingResponseLevel,
  promoted = false,
  onWrite,
}: PidResultsPanelProps) {
  const pidLabels = run.pid_parameter_labels ?? {
    proportional: "P",
    integral: "I",
    derivative: "D",
  };

  return (
    <section
      className={
        promoted
          ? "mb-8 rounded-xl border border-emerald-800/70 bg-gradient-to-br from-emerald-950/40 via-slate-950/20 to-slate-900/40 p-5 shadow-lg shadow-emerald-950/20"
          : "mb-6"
      }
    >
      <div className="flex items-start justify-between gap-4">
        <div>
          <h2 className="mb-1 text-sm font-semibold uppercase tracking-wide text-slate-300">
            Calculated results
          </h2>
          {promoted && run.results.length > 0 && (
            <p className="text-sm text-slate-300">
              Choose a response level to review the exact PID values before
              writing them to the controller.
            </p>
          )}
        </div>
        {promoted && run.results.length > 0 && (
          <span className="shrink-0 rounded-full border border-emerald-700/70 bg-emerald-950/60 px-3 py-1 text-xs font-medium text-emerald-300">
            Ready to review
          </span>
        )}
      </div>

      {run.results.length === 0 ? (
        <p className="mt-3 text-sm text-slate-500">
          No results were calculated for this tune.
        </p>
      ) : (
        <div className="mt-4 overflow-x-auto rounded-lg border border-slate-800">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
              <tr>
                <th className="px-4 py-2 font-medium">Response level</th>
                <th className="px-4 py-2 font-medium">
                  {pidLabels.proportional}
                </th>
                <th className="px-4 py-2 font-medium">{pidLabels.integral}</th>
                <th className="px-4 py-2 font-medium">
                  {pidLabels.derivative}
                </th>
                <th className="px-4 py-2 font-medium">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-800">
              {run.results.map((result) => (
                <ResultRow
                  key={result.response_level}
                  result={result}
                  eligibility={eligibility}
                  writePending={writePending}
                  writingResponseLevel={writingResponseLevel}
                  onWrite={onWrite}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
      {!eligibility.eligible && eligibility.reason && (
        <p className="mt-3 text-xs text-slate-500">
          PID changes unavailable: {eligibility.reason}
        </p>
      )}
    </section>
  );
}

function ResultRow({
  result,
  eligibility,
  writePending,
  writingResponseLevel,
  onWrite,
}: {
  readonly result: RunResult;
  readonly eligibility: WriteEligibility;
  readonly writePending: boolean;
  readonly writingResponseLevel?: ResponseLevel;
  readonly onWrite: (result: RunResult) => void;
}) {
  const isWriting =
    writePending && writingResponseLevel === result.response_level;

  return (
    <tr>
      <td className="px-4 py-3 font-medium">
        {RESPONSE_LEVEL_LABELS[result.response_level]}
      </td>
      <td className="px-4 py-3 font-mono">
        {formatNumber(result.proportional)}
      </td>
      <td className="px-4 py-3 font-mono">{formatNumber(result.integral)}</td>
      <td className="px-4 py-3 font-mono">{formatNumber(result.derivative)}</td>
      <td className="px-4 py-3">
        <Button
          variant="primary"
          disabled={!eligibility.eligible || writePending}
          title={eligibility.reason}
          onClick={() => {
            if (eligibility.eligible) onWrite(result);
          }}
        >
          {isWriting ? "Writing…" : "Review & write"}
        </Button>
      </td>
    </tr>
  );
}
