import { userFacingErrorMessage } from "../../api/errors";
import type {
  MvActuation,
  RunDetailResponse,
  RunStreamState,
  ResponseLevel,
  SampleResponse,
} from "../../api/runs";
import {
  CONTROLLER_TYPE_LABELS,
  DIRECTION_LABELS,
  DRIVER_LABELS,
  MV_ACTUATION_KIND_LABELS,
  MV_ACTUATION_STATUS_LABELS,
  OUTCOME_LABELS,
  PROCESS_TYPE_LABELS,
  RESPONSE_LEVEL_LABELS,
  SAMPLE_QUALITY_LABELS,
} from "../../lib/enumLabels";
import { TrendChart } from "../../components/TrendChart";
import {
  Badge,
  Button,
  ErrorBanner,
  Field,
  Section,
  TextAreaField,
} from "../../components/ui";
import type { TrendPoint } from "../../lib/trend";
import {
  type RunResult,
  type RunWrite,
  type WriteEligibility,
  writeKey,
} from "./runDetailHelpers";

function num(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  return String(Number(value.toFixed(4)));
}

function dateTime(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString() : "—";
}

export interface RunErrorItem {
  readonly key: string;
  readonly error: unknown;
  readonly fallback: string;
}

export function RunDetailErrors({
  errors,
}: {
  readonly errors: readonly RunErrorItem[];
}) {
  return (
    <>
      {errors.map(({ key, error, fallback }) =>
        error ? (
          <ErrorBanner
            key={key}
            message={userFacingErrorMessage(error, fallback)}
          />
        ) : null,
      )}
    </>
  );
}

function LiveProgress({ stream }: { readonly stream: RunStreamState }) {
  const latest = stream.samples.at(-1);

  return (
    <div className="mb-6 rounded-lg border border-slate-700 bg-slate-900/60 px-4 py-3 text-sm">
      <p className="text-slate-300">
        Tune in progress — collecting live measurements.
        {stream.reconnecting && (
          <span className="ml-2 text-amber-400">
            Connection interrupted — retrying…
          </span>
        )}
      </p>
      {latest ? (
        <p className="mt-2 font-mono text-slate-400">
          Tick {latest.tick_index}: PV {num(latest.sample.pv)}, MV{" "}
          {num(latest.state.mv_value_current)}, cycles{" "}
          {latest.state.cycles_completed} completed /{" "}
          {latest.state.cycles_remaining} remaining
        </p>
      ) : (
        <p className="mt-2 text-slate-500">
          {stream.initialReadings
            ? "Initial readings captured; waiting for the first measurement."
            : "No measurements recorded yet."}
        </p>
      )}
    </div>
  );
}

function TrendSection({
  points,
  pollIntervalMs,
}: {
  readonly points: readonly TrendPoint[];
  readonly pollIntervalMs: number | null | undefined;
}) {
  return (
    <section className="mb-6">
      <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        Trend
      </h2>
      {points.length === 0 ? (
        <p className="text-sm text-slate-500">No measurements recorded yet.</p>
      ) : (
        <TrendChart points={points} pollIntervalMs={pollIntervalMs} />
      )}
    </section>
  );
}

function SummarySection({ run }: { readonly run: RunDetailResponse }) {
  return (
    <Section title="Summary">
      <Field label="Tag name" value={run.tag_name} />
      <Field
        label="Outcome"
        value={
          <Badge tone={outcomeTone[run.outcome]}>
            {OUTCOME_LABELS[run.outcome]}
          </Badge>
        }
      />
      <Field label="Driver" value={DRIVER_LABELS[run.driver]} />
      <Field
        label="Template"
        value={
          <>
            {run.template_name}{" "}
            <Badge tone={originTone[run.template_origin]}>
              {run.template_origin}
            </Badge>
          </>
        }
      />
      <Field label="Started" value={dateTime(run.started_at)} />
      <Field label="Completed" value={dateTime(run.completed_at)} />
      {run.failure_reason && (
        <Field label="End reason" value={run.failure_reason} full />
      )}
      {run.restore_status && (
        <Field
          label="Restore status"
          value={
            <Badge tone={restoreTone[run.restore_status]}>
              {run.restore_status}
            </Badge>
          }
        />
      )}
      {run.restore_detail && (
        <Field label="Restore detail" value={run.restore_detail} full />
      )}
    </Section>
  );
}

const originTone = {
  builtin: "success",
  catalog: "neutral",
  user: "warning",
} as const;

const outcomeTone = {
  running: "neutral",
  completed: "success",
  failed: "error",
  aborted: "warning",
} as const;

const restoreTone = {
  confirmed: "success",
  incomplete: "warning",
} as const;

function NotesSection({
  notes,
  notesDirty,
  savePending,
  clearPending,
  onNotesChange,
  onSave,
  onClear,
}: {
  readonly notes: string;
  readonly notesDirty: boolean;
  readonly savePending: boolean;
  readonly clearPending: boolean;
  readonly onNotesChange: (value: string) => void;
  readonly onSave: () => void;
  readonly onClear: () => void;
}) {
  return (
    <section className="mb-6">
      <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        Notes
      </h2>
      <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-5">
        <TextAreaField
          label="Run notes"
          value={notes}
          onChange={onNotesChange}
          full
          placeholder="Optional context, observations, or follow-up actions"
          hint="Notes can be changed while the tune is active or after it finishes."
        />
        <div className="mt-3 flex gap-2">
          <Button
            variant="primary"
            disabled={!notesDirty || savePending}
            onClick={onSave}
          >
            {savePending ? "Saving…" : "Save notes"}
          </Button>
          <Button
            variant="danger"
            disabled={
              clearPending || (!notesDirty && notes.trim().length === 0)
            }
            onClick={onClear}
          >
            {clearPending ? "Clearing…" : "Clear notes"}
          </Button>
        </div>
      </div>
    </section>
  );
}

function ConfigurationSection({ run }: { readonly run: RunDetailResponse }) {
  return (
    <Section title="Test configuration">
      <Field
        label="Process type"
        value={PROCESS_TYPE_LABELS[run.config.process_type]}
      />
      <Field
        label="Controller type"
        value={CONTROLLER_TYPE_LABELS[run.config.controller_type]}
      />
      <Field
        label="Relay amplitude"
        value={`${num(run.config.relay_amp_percent)}%`}
      />
      <Field
        label="Cycles (skip / count)"
        value={`${run.config.num_cycles_skip} / ${run.config.num_cycles_count}`}
      />
      <Field
        label="Noise protection"
        value={`${run.config.noise_protection_secs}s`}
      />
      <Field
        label="MRFT delay padding"
        value={`${run.config.mrft_delay_secs}s`}
      />
    </Section>
  );
}

function InitialReadingsSection({
  readings,
}: {
  readonly readings: NonNullable<RunDetailResponse["initial_readings"]>;
}) {
  return (
    <Section title="Initial readings">
      <Field label="PV initial" value={num(readings.pv_ini)} />
      <Field label="MV initial" value={num(readings.mv_ini)} />
      <Field
        label="PV range"
        value={`${num(readings.pv_range_low)} – ${num(readings.pv_range_high)}`}
      />
      <Field
        label="MV range"
        value={`${num(readings.mv_range_low)} – ${num(readings.mv_range_high)}`}
      />
      <Field
        label="Controller direction"
        value={DIRECTION_LABELS[readings.controller_direction]}
      />
      <Field label="Setpoint initial" value={num(readings.setpoint_ini)} />
      <Field label="Mode (raw)" value={readings.mode_raw ?? "—"} />
      <Field
        label="Mode attribute (raw)"
        value={readings.mode_attribute_raw ?? "—"}
      />
    </Section>
  );
}

function MvActuationSection({
  actuations,
}: {
  readonly actuations: readonly MvActuation[];
}) {
  return (
    <section className="mb-6">
      <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        MV actuation verification
      </h2>
      {actuations.length === 0 ? (
        <p className="text-sm text-slate-500">
          No OPC DA MV commands were recorded for this tune.
        </p>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-slate-800">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
              <tr>
                <th className="px-4 py-2 font-medium">Command</th>
                <th className="px-4 py-2 font-medium">Commanded at</th>
                <th className="px-4 py-2 font-medium">Target MV</th>
                <th className="px-4 py-2 font-medium">Readback</th>
                <th className="px-4 py-2 font-medium">Tolerance</th>
                <th className="px-4 py-2 font-medium">Due</th>
                <th className="px-4 py-2 font-medium">Attempts</th>
                <th className="px-4 py-2 font-medium">Status</th>
                <th className="px-4 py-2 font-medium">Detail</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-800">
              {actuations.map((actuation) => (
                <MvActuationRow key={actuation.id} actuation={actuation} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function MvActuationRow({ actuation }: { readonly actuation: MvActuation }) {
  const status = actuationStatus(actuation.status);
  return (
    <tr>
      <td className="px-4 py-3">
        <div className="font-medium">
          {MV_ACTUATION_KIND_LABELS[actuation.kind]}
        </div>
        <div className="text-xs text-slate-500">#{actuation.sequence}</div>
      </td>
      <td className="px-4 py-3 text-slate-400">
        <div>{dateTime(actuation.commanded_at)}</div>
        <div className="text-xs">
          check {dateTime(actuation.last_checked_at)}
        </div>
      </td>
      <td className="px-4 py-3 font-mono">{num(actuation.target_mv)}</td>
      <td className="px-4 py-3 font-mono">
        <div>{num(actuation.readback_mv)}</div>
        {actuation.readback_quality && (
          <div className="font-sans text-xs text-slate-400">
            {SAMPLE_QUALITY_LABELS[actuation.readback_quality]}
          </div>
        )}
      </td>
      <td className="px-4 py-3 font-mono">{num(actuation.tolerance)}</td>
      <td className="px-4 py-3 text-slate-400">
        {dateTime(actuation.confirmation_due_at)}
      </td>
      <td className="px-4 py-3">{actuation.attempt_count}</td>
      <td className="px-4 py-3">
        <Badge tone={status.tone}>{status.label}</Badge>
      </td>
      <td className="max-w-sm px-4 py-3 text-slate-400">
        {actuation.detail ?? "—"}
      </td>
    </tr>
  );
}

function actuationStatus(status: MvActuation["status"]) {
  const tone = actuationStatusTone(status);
  return { tone, label: MV_ACTUATION_STATUS_LABELS[status] };
}

function actuationStatusTone(
  status: MvActuation["status"],
): "success" | "error" | "warning" | "neutral" {
  switch (status) {
    case "confirmed":
      return "success";
    case "failed":
      return "error";
    case "unverified":
      return "warning";
    case "pending":
    case "superseded":
      return "neutral";
  }
}

function CalculatedResultsSection({
  run,
  eligibility,
  writePending,
  writingResponseLevel,
  onWrite,
}: {
  readonly run: RunDetailResponse;
  readonly eligibility: WriteEligibility;
  readonly writePending: boolean;
  readonly writingResponseLevel?: ResponseLevel;
  readonly onWrite: (result: RunResult) => void;
}) {
  return (
    <section className="mb-6">
      <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        Calculated results
      </h2>
      {run.results.length === 0 ? (
        <p className="text-sm text-slate-500">
          No results were calculated for this tune.
        </p>
      ) : (
        <div className="overflow-hidden rounded-lg border border-slate-800">
          <table className="w-full text-left text-sm">
            <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
              <tr>
                <th className="px-4 py-2 font-medium">Response level</th>
                <th className="px-4 py-2 font-medium">Kp</th>
                <th className="px-4 py-2 font-medium">Ti (min)</th>
                <th className="px-4 py-2 font-medium">Td (min)</th>
                <th className="px-4 py-2 font-medium">P</th>
                <th className="px-4 py-2 font-medium">I</th>
                <th className="px-4 py-2 font-medium">D</th>
                <th className="px-4 py-2 font-medium">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-800">
              {run.results.map((result) => (
                <ResultRow
                  key={result.response_level}
                  result={result}
                  run={run}
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
        <p className="mt-2 text-xs text-slate-500">
          PID changes unavailable: {eligibility.reason}
        </p>
      )}
    </section>
  );
}

function ResultRow({
  result,
  run,
  eligibility,
  writePending,
  writingResponseLevel,
  onWrite,
}: {
  readonly result: RunResult;
  readonly run: RunDetailResponse;
  readonly eligibility: WriteEligibility;
  readonly writePending: boolean;
  readonly writingResponseLevel?: ResponseLevel;
  readonly onWrite: (result: RunResult) => void;
}) {
  const isWriting =
    writePending && writingResponseLevel === result.response_level;

  function handleWrite() {
    const tags = run.pid_constant_tags;
    if (!eligibility.eligible || !tags) return;

    const confirmed = window.confirm(
      `Apply ${RESPONSE_LEVEL_LABELS[result.response_level]} PID constants to tag "${run.tag_name}"?\n\n` +
        `${tags.proportional}: ${num(result.proportional)}\n` +
        `${tags.integral}: ${num(result.integral)}\n` +
        `${tags.derivative}: ${num(result.derivative)}`,
    );
    if (confirmed) onWrite(result);
  }

  return (
    <tr>
      <td className="px-4 py-3 font-medium">
        {RESPONSE_LEVEL_LABELS[result.response_level]}
      </td>
      <td className="px-4 py-3 font-mono">{num(result.kp)}</td>
      <td className="px-4 py-3 font-mono">{num(result.ti_minutes)}</td>
      <td className="px-4 py-3 font-mono">{num(result.td_minutes)}</td>
      <td className="px-4 py-3 font-mono">{num(result.proportional)}</td>
      <td className="px-4 py-3 font-mono">{num(result.integral)}</td>
      <td className="px-4 py-3 font-mono">{num(result.derivative)}</td>
      <td className="px-4 py-3">
        <Button
          variant="primary"
          disabled={!eligibility.eligible || writePending}
          title={eligibility.reason}
          onClick={handleWrite}
        >
          {isWriting ? "Writing…" : "Apply"}
        </Button>
      </td>
    </tr>
  );
}

function WriteHistorySection({
  run,
  eligibility,
  canRevertLastWrite,
  revertPending,
  onRevert,
}: {
  readonly run: RunDetailResponse;
  readonly eligibility: WriteEligibility;
  readonly canRevertLastWrite: boolean;
  readonly revertPending: boolean;
  readonly onRevert: () => void;
}) {
  return (
    <section className="mb-6">
      <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
        PID change history
      </h2>
      {run.writes.length === 0 ? (
        <p className="text-sm text-slate-500">
          No PID settings were applied during this tune.
        </p>
      ) : (
        <WriteTable
          run={run}
          eligibility={eligibility}
          canRevertLastWrite={canRevertLastWrite}
          revertPending={revertPending}
          onRevert={onRevert}
        />
      )}
      <WriteErrors writes={run.writes} />
    </section>
  );
}

function WriteTable({
  run,
  eligibility,
  canRevertLastWrite,
  revertPending,
  onRevert,
}: {
  readonly run: RunDetailResponse;
  readonly eligibility: WriteEligibility;
  readonly canRevertLastWrite: boolean;
  readonly revertPending: boolean;
  readonly onRevert: () => void;
}) {
  return (
    <div className="overflow-x-auto rounded-lg border border-slate-800">
      <table className="w-full text-left text-sm">
        <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
          <tr>
            <th className="px-4 py-2 font-medium">Action</th>
            <th className="px-4 py-2 font-medium">Level</th>
            <th className="px-4 py-2 font-medium">Changed at</th>
            <th className="px-4 py-2 font-medium">Previous values</th>
            <th className="px-4 py-2 font-medium">Applied values</th>
            <th className="px-4 py-2 font-medium">Read-back values</th>
            <th className="px-4 py-2 font-medium">Success</th>
            <th className="px-4 py-2 font-medium">Rollback</th>
            <th className="px-4 py-2 font-medium">Actions</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-800">
          {run.writes.map((write) => (
            <WriteRow
              key={writeKey(write)}
              write={write}
              run={run}
              eligibility={eligibility}
              isLastWrite={write === run.writes.at(-1)}
              canRevertLastWrite={canRevertLastWrite}
              revertPending={revertPending}
              onRevert={onRevert}
            />
          ))}
        </tbody>
      </table>
    </div>
  );
}

function WriteRow({
  write,
  run,
  eligibility,
  isLastWrite,
  canRevertLastWrite,
  revertPending,
  onRevert,
}: {
  readonly write: RunWrite;
  readonly run: RunDetailResponse;
  readonly eligibility: WriteEligibility;
  readonly isLastWrite: boolean;
  readonly canRevertLastWrite: boolean;
  readonly revertPending: boolean;
  readonly onRevert: () => void;
}) {
  function handleRevert() {
    const tags = run.pid_constant_tags;
    if (!eligibility.eligible || !tags) return;

    const confirmed = window.confirm(
      `Restore the previous PID values on tag "${run.tag_name}"?\n\n` +
        `${tags.proportional}: ${num(write.proportional_previous)}\n` +
        `${tags.integral}: ${num(write.integral_previous)}\n` +
        `${tags.derivative}: ${num(write.derivative_previous)}`,
    );
    if (confirmed) onRevert();
  }

  return (
    <tr>
      <td className="px-4 py-3">
        {write.kind === "write" ? "Apply" : "Restore"}
      </td>
      <td className="px-4 py-3">
        {RESPONSE_LEVEL_LABELS[write.response_level]}
      </td>
      <td className="px-4 py-3 text-slate-400">{dateTime(write.written_at)}</td>
      <td className="px-4 py-3 font-mono text-slate-400">
        {num(write.proportional_previous)} / {num(write.integral_previous)} /{" "}
        {num(write.derivative_previous)}
      </td>
      <td className="px-4 py-3 font-mono">
        {num(write.proportional_written)} / {num(write.integral_written)} /{" "}
        {num(write.derivative_written)}
      </td>
      <td className="px-4 py-3 font-mono text-slate-400">
        {num(write.proportional_readback)} / {num(write.integral_readback)} /{" "}
        {num(write.derivative_readback)}
      </td>
      <td className="px-4 py-3">
        <Badge tone={write.success ? "success" : "error"}>
          {write.success ? "Successful" : "Failed"}
        </Badge>
      </td>
      <td className="px-4 py-3">
        <RollbackBadge state={write.rollback_state} />
      </td>
      <td className="px-4 py-3">
        {isLastWrite && canRevertLastWrite ? (
          <Button
            variant="danger"
            disabled={revertPending}
            title={eligibility.reason}
            onClick={handleRevert}
          >
            {revertPending ? "Restoring…" : "Restore previous values"}
          </Button>
        ) : (
          <span className="text-slate-500">—</span>
        )}
      </td>
    </tr>
  );
}

function RollbackBadge({
  state,
}: {
  readonly state: RunWrite["rollback_state"];
}) {
  const badge = rollbackBadge(state);
  if (!badge) return <span className="text-slate-500">—</span>;
  return <Badge tone={badge.tone}>{badge.label}</Badge>;
}

function rollbackBadge(state: RunWrite["rollback_state"]) {
  switch (state) {
    case "succeeded":
      return { tone: "success" as const, label: "Restored" };
    case "failed":
      return { tone: "error" as const, label: "Could not restore" };
    default:
      return null;
  }
}

function WriteErrors({ writes }: { readonly writes: readonly RunWrite[] }) {
  const failedWrites = writes.filter((write) => write.error_message);
  if (failedWrites.length === 0) return null;

  return (
    <div className="mt-3 space-y-2">
      {failedWrites.map((write) => (
        <ErrorBanner
          key={`${writeKey(write)}:error`}
          message={`${RESPONSE_LEVEL_LABELS[write.response_level]}: ${failureMessage(write)}`}
        />
      ))}
    </div>
  );
}

function failureMessage(write: RunWrite): string {
  if (write.kind === "revert") {
    return "The previous PID values could not be restored. Check the OPC connection and try again.";
  }
  return "The PID settings could not be applied. Check the OPC connection and try again.";
}

export function RunDetailContent({
  run,
  isRunning,
  stream,
  initialReadings,
  trendSamples,
  trendPoints,
  trendPollIntervalMs,
  eligibility,
  canRevertLastWrite,
  notes,
  notesDirty,
  savePending,
  clearPending,
  writePending,
  writingResponseLevel,
  revertPending,
  onNotesChange,
  onSaveNotes,
  onClearNotes,
  onWrite,
  onRevert,
}: {
  readonly run: RunDetailResponse;
  readonly isRunning: boolean;
  readonly stream: RunStreamState;
  readonly initialReadings: RunDetailResponse["initial_readings"];
  readonly trendSamples: readonly SampleResponse[];
  readonly trendPoints: readonly TrendPoint[];
  readonly trendPollIntervalMs: number | null | undefined;
  readonly eligibility: WriteEligibility;
  readonly canRevertLastWrite: boolean;
  readonly notes: string;
  readonly notesDirty: boolean;
  readonly savePending: boolean;
  readonly clearPending: boolean;
  readonly writePending: boolean;
  readonly writingResponseLevel?: ResponseLevel;
  readonly revertPending: boolean;
  readonly onNotesChange: (value: string) => void;
  readonly onSaveNotes: () => void;
  readonly onClearNotes: () => void;
  readonly onWrite: (result: RunResult) => void;
  readonly onRevert: () => void;
}) {
  return (
    <>
      {isRunning && <LiveProgress stream={stream} />}
      <TrendSection points={trendPoints} pollIntervalMs={trendPollIntervalMs} />
      <SummarySection run={run} />
      <NotesSection
        notes={notes}
        notesDirty={notesDirty}
        savePending={savePending}
        clearPending={clearPending}
        onNotesChange={onNotesChange}
        onSave={onSaveNotes}
        onClear={onClearNotes}
      />
      <ConfigurationSection run={run} />
      {initialReadings && <InitialReadingsSection readings={initialReadings} />}
      {run.driver === "opcda" && (
        <MvActuationSection actuations={run.mv_actuations} />
      )}
      <CalculatedResultsSection
        run={run}
        eligibility={eligibility}
        writePending={writePending}
        writingResponseLevel={writingResponseLevel}
        onWrite={onWrite}
      />
      <WriteHistorySection
        run={run}
        eligibility={eligibility}
        canRevertLastWrite={canRevertLastWrite}
        revertPending={revertPending}
        onRevert={onRevert}
      />
      <p className="text-sm text-slate-500">
        {trendSamples.length} measurements{" "}
        {isRunning ? "recorded so far" : "were recorded"} for this tune.
      </p>
    </>
  );
}
