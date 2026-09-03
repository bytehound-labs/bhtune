import { userFacingErrorMessage } from "../../api/errors";
import type {
  RunDetailResponse,
  RunStreamState,
  ResponseLevel,
  SampleResponse,
} from "../../api/runs";
import {
  CONTROLLER_TYPE_LABELS,
  DIRECTION_LABELS,
  DRIVER_LABELS,
  OUTCOME_LABELS,
  PROCESS_TYPE_LABELS,
  RESPONSE_LEVEL_LABELS,
} from "../../lib/enumLabels";
import { TrendChart } from "../../components/TrendChart";
import {
  Badge,
  Button,
  ErrorBanner,
  Field,
  CollapsibleSection,
  Section,
  TextAreaField,
} from "../../components/ui";
import type { TrendPoint } from "../../lib/trend";
import {
  formatNumber,
  type ValidRunResult,
  type RunWrite,
  type WriteEligibility,
  writeFailureMessage,
  writeKey,
} from "./runDetailHelpers";
import { PidResultsPanel } from "./PidResultsPanel";
import { SamplingDiagnosticsSection } from "./SamplingDiagnosticsSection";

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
  demo = false,
}: {
  readonly errors: readonly RunErrorItem[];
  readonly demo?: boolean;
}) {
  return (
    <>
      {errors.map(({ key, error, fallback }) =>
        error ? (
          <ErrorBanner
            key={key}
            message={userFacingErrorMessage(error, fallback, demo)}
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
          Tick {latest.tick_index}: PV {formatNumber(latest.sample.pv)}, MV{" "}
          {formatNumber(latest.state.mv_value_current)}, cycles{" "}
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
    <CollapsibleSection title="Trend">
      {points.length === 0 ? (
        <p className="text-sm text-slate-500">No measurements recorded yet.</p>
      ) : (
        <TrendChart points={points} pollIntervalMs={pollIntervalMs} />
      )}
    </CollapsibleSection>
  );
}

function SummarySection({
  run,
  demo,
}: {
  readonly run: RunDetailResponse;
  readonly demo: boolean;
}) {
  return (
    <Section title="Summary" collapsible defaultOpen>
      <Field
        label={demo ? "Tune" : "Tag name"}
        value={demo ? "Simulator demo" : run.tag_name}
      />
      <Field
        label="Outcome"
        value={
          <Badge tone={outcomeTone[run.outcome]}>
            {OUTCOME_LABELS[run.outcome]}
          </Badge>
        }
      />
      <Field
        label="Driver"
        value={demo ? "Simulator demo" : DRIVER_LABELS[run.driver]}
      />
      <Field
        label="Template"
        value={
          <>
            {run.template_name}{" "}
            {!demo && (
              <Badge tone={originTone[run.template_origin]}>
                {run.template_origin}
              </Badge>
            )}
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
    <CollapsibleSection title="Notes">
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
    </CollapsibleSection>
  );
}

function ConfigurationSection({ run }: { readonly run: RunDetailResponse }) {
  return (
    <Section title="Test configuration" collapsible defaultOpen>
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
        value={`${formatNumber(run.config.relay_amp_percent)}%`}
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
    <Section title="Initial readings" collapsible defaultOpen>
      <Field label="PV initial" value={formatNumber(readings.pv_ini)} />
      <Field label="MV initial" value={formatNumber(readings.mv_ini)} />
      <Field
        label="PV range"
        value={`${formatNumber(readings.pv_range_low)} – ${formatNumber(readings.pv_range_high)}`}
      />
      <Field
        label="MV range"
        value={`${formatNumber(readings.mv_range_low)} – ${formatNumber(readings.mv_range_high)}`}
      />
      <Field
        label="Controller direction"
        value={DIRECTION_LABELS[readings.controller_direction]}
      />
      <Field
        label="Setpoint initial"
        value={formatNumber(readings.setpoint_ini)}
      />
      <Field label="Mode (raw)" value={readings.mode_raw ?? "—"} />
      <Field
        label="Mode attribute (raw)"
        value={readings.mode_attribute_raw ?? "—"}
      />
    </Section>
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
  readonly onRevert: (write: RunWrite) => void;
}) {
  return (
    <CollapsibleSection title="PID change history">
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
    </CollapsibleSection>
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
  readonly onRevert: (write: RunWrite) => void;
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
  eligibility,
  isLastWrite,
  canRevertLastWrite,
  revertPending,
  onRevert,
}: {
  readonly write: RunWrite;
  readonly eligibility: WriteEligibility;
  readonly isLastWrite: boolean;
  readonly canRevertLastWrite: boolean;
  readonly revertPending: boolean;
  readonly onRevert: (write: RunWrite) => void;
}) {
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
        {formatNumber(write.proportional_previous)} /{" "}
        {formatNumber(write.integral_previous)} /{" "}
        {formatNumber(write.derivative_previous)}
      </td>
      <td className="px-4 py-3 font-mono">
        {formatNumber(write.proportional_written)} /{" "}
        {formatNumber(write.integral_written)} /{" "}
        {formatNumber(write.derivative_written)}
      </td>
      <td className="px-4 py-3 font-mono text-slate-400">
        {formatNumber(write.proportional_readback)} /{" "}
        {formatNumber(write.integral_readback)} /{" "}
        {formatNumber(write.derivative_readback)}
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
            onClick={() => onRevert(write)}
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
          message={`${RESPONSE_LEVEL_LABELS[write.response_level]}: ${writeFailureMessage(write)}`}
        />
      ))}
    </div>
  );
}

export function RunDetailContent({
  run,
  demo,
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
  readonly demo: boolean;
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
  readonly onWrite: (result: ValidRunResult) => void;
  readonly onRevert: (write: RunWrite) => void;
}) {
  return (
    <>
      {isRunning && <LiveProgress stream={stream} />}
      {run.results.length > 0 && (
        <PidResultsPanel
          run={run}
          demo={demo}
          eligibility={eligibility}
          writePending={writePending}
          writingResponseLevel={writingResponseLevel}
          promoted
          onWrite={onWrite}
        />
      )}
      <TrendSection points={trendPoints} pollIntervalMs={trendPollIntervalMs} />
      <SummarySection run={run} demo={demo} />
      {!demo && (
        <NotesSection
          notes={notes}
          notesDirty={notesDirty}
          savePending={savePending}
          clearPending={clearPending}
          onNotesChange={onNotesChange}
          onSave={onSaveNotes}
          onClear={onClearNotes}
        />
      )}
      <ConfigurationSection run={run} />
      {initialReadings && <InitialReadingsSection readings={initialReadings} />}
      {run.results.length === 0 && (
        <PidResultsPanel
          run={run}
          demo={demo}
          eligibility={eligibility}
          writePending={writePending}
          writingResponseLevel={writingResponseLevel}
          onWrite={onWrite}
        />
      )}
      {!demo && (
        <WriteHistorySection
          run={run}
          eligibility={eligibility}
          canRevertLastWrite={canRevertLastWrite}
          revertPending={revertPending}
          onRevert={onRevert}
        />
      )}
      <SamplingDiagnosticsSection timing={run.timing_metrics} />
      <p className="text-sm text-slate-500">
        {trendSamples.length} measurements{" "}
        {isRunning ? "recorded so far" : "were recorded"} for this tune.
      </p>
    </>
  );
}
