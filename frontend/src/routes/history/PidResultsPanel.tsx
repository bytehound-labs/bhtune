import type { RunDetailResponse, ResponseLevel } from "../../api/runs";
import { RESPONSE_LEVEL_LABELS } from "../../lib/enumLabels";
import { Badge, Button, CollapsibleSection } from "../../components/ui";
import {
  formatNumber,
  invalidResultReason,
  isValidRunResult,
  type RunResult,
  type ValidRunResult,
  type WriteEligibility,
} from "./runDetailHelpers";

interface PidResultsPanelProps {
  readonly run: RunDetailResponse;
  readonly demo?: boolean;
  readonly eligibility: WriteEligibility;
  readonly writePending: boolean;
  readonly writingResponseLevel?: ResponseLevel;
  readonly promoted?: boolean;
  readonly onWrite: (result: ValidRunResult) => void;
}

export function PidResultsPanel({
  run,
  demo = false,
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
    <CollapsibleSection
      title="Calculated results"
      defaultOpen
      className={
        promoted
          ? "mb-8 rounded-xl border border-emerald-800/70 bg-gradient-to-br from-emerald-950/40 via-slate-950/20 to-slate-900/40 p-5 shadow-lg shadow-emerald-950/20"
          : "mb-6"
      }
      trailing={
        promoted && run.results.length > 0 ? (
          <span className="rounded-full border border-emerald-700/70 bg-emerald-950/60 px-3 py-1 text-xs font-medium text-emerald-300">
            Ready to review
          </span>
        ) : undefined
      }
    >
      {promoted && run.results.length > 0 && (
        <p className="mb-4 text-sm text-slate-300">
          {demo
            ? "Compare the synthetic response levels. Demo results cannot be written to a controller."
            : "Choose a response level to review the exact PID values before writing them to the controller."}
        </p>
      )}

      {run.results.length === 0 ? (
        <p className="text-sm text-slate-500">
          No results were calculated for this tune.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-slate-800">
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
                <th className="px-4 py-2 font-medium">Status</th>
                {!demo && <th className="px-4 py-2 font-medium">Actions</th>}
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-800">
              {run.results.map((result) => (
                <ResultRow
                  key={result.response_level}
                  result={result}
                  demo={demo}
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
      {!demo && !eligibility.eligible && eligibility.reason && (
        <p className="mt-3 text-xs text-slate-500">
          PID changes unavailable: {eligibility.reason}
        </p>
      )}
    </CollapsibleSection>
  );
}

function ResultRow({
  result,
  demo,
  eligibility,
  writePending,
  writingResponseLevel,
  onWrite,
}: {
  readonly result: RunResult;
  readonly demo: boolean;
  readonly eligibility: WriteEligibility;
  readonly writePending: boolean;
  readonly writingResponseLevel?: ResponseLevel;
  readonly onWrite: (result: ValidRunResult) => void;
}) {
  const isWriting =
    writePending && writingResponseLevel === result.response_level;
  const isValid = isValidRunResult(result);
  const resultReason = isValid ? undefined : invalidResultReason(result);
  const canWrite = eligibility.eligible && isValid;

  return (
    <tr>
      <td className="px-4 py-3 font-medium">
        {RESPONSE_LEVEL_LABELS[result.response_level]}
      </td>
      <td className="px-4 py-3 font-mono">
        {isValid ? formatNumber(result.proportional) : "—"}
      </td>
      <td className="px-4 py-3 font-mono">
        {isValid ? formatNumber(result.integral) : "—"}
      </td>
      <td className="px-4 py-3 font-mono">
        {isValid ? formatNumber(result.derivative) : "—"}
      </td>
      <td className="px-4 py-3 align-top">
        {isValid ? (
          <Badge tone="success">Valid</Badge>
        ) : (
          <div className="max-w-xs space-y-1">
            <Badge tone="error">Invalid</Badge>
            <p className="text-xs text-red-300">{resultReason}</p>
          </div>
        )}
      </td>
      {!demo && (
        <td className="px-4 py-3">
          <Button
            variant="primary"
            disabled={!canWrite || writePending}
            title={
              isValid
                ? eligibility.reason
                : `Calculated result unavailable: ${resultReason}`
            }
            onClick={() => {
              if (canWrite) onWrite(result);
            }}
          >
            {isWriting ? "Writing…" : "Review & write"}
          </Button>
        </td>
      )}
    </tr>
  );
}
