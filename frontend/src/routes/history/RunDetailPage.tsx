import { useEffect, useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import {
  runExportUrl,
  useCancelRun,
  useDeleteRunNotes,
  useDeleteRun,
  useRevertRun,
  useRun,
  useRunStream,
  useUpdateRunNotes,
  useWriteRun,
  type SampleResponse,
  type RunDetailResponse,
} from "../../api/runs";
import { userFacingErrorMessage } from "../../api/errors";
import type { DuplicateRunState } from "../runs/NewRunPage";
import {
  CONTROLLER_TYPE_LABELS,
  DIRECTION_LABELS,
  DRIVER_LABELS,
  OUTCOME_LABELS,
  PROCESS_TYPE_LABELS,
  RESPONSE_LEVEL_LABELS,
} from "../../lib/enumLabels";
import {
  Badge,
  Button,
  ErrorBanner,
  Field,
  LoadingState,
  PageHeading,
  Section,
  TextAreaField,
} from "../../components/ui";
import { TrendChart } from "../../components/TrendChart";
import { composeTrendPoints } from "../../lib/trend";

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

const EMPTY_TREND_SAMPLES: readonly SampleResponse[] = [];

/** Trims a float to a stable, readable precision without trailing zeros. */
function num(value: number | null | undefined): string {
  if (value === null || value === undefined) return "—";
  return String(Number(value.toFixed(4)));
}

function dateTime(value: string | null | undefined): string {
  return value ? new Date(value).toLocaleString() : "—";
}

/**
 * Whether this tune is eligible for post-run PID changes, mirroring
 * `routes::runs::require_writable_run`'s checks client-side so the buttons can be disabled
 * with a reason *before* a request is ever made, rather than only discovering ineligibility
 * from a failed call (`api-post-run-write`, `ui-post-run-write`). Both actions share the same
 * eligibility on the server, so one check covers the Apply and Restore buttons alike.
 */
function writeEligibility(run: RunDetailResponse): {
  eligible: boolean;
  reason?: string;
} {
  if (run.outcome === "running") {
    return {
      eligible: false,
      reason: "This tune is still in progress; wait for it to finish.",
    };
  }
  if (run.driver !== "opcda") {
    return {
      eligible: false,
      reason: `${DRIVER_LABELS[run.driver]} tunes have no live loop to change PID settings on.`,
    };
  }
  if (!run.pid_constant_tags) {
    return {
      eligible: false,
      reason: "This tune's template has no PID constant tags configured.",
    };
  }
  if (!run.opc_server || !run.bridge_host) {
    return {
      eligible: false,
      reason: "This tune has no recorded OPC server/bridge host connection.",
    };
  }
  return { eligible: true };
}

export function RunDetailPage() {
  const { id } = useParams<{ id: string }>();
  const runId = Number(id);
  const navigate = useNavigate();
  const run = useRun(runId);
  const cancelRun = useCancelRun();
  const updateNotes = useUpdateRunNotes();
  const deleteNotes = useDeleteRunNotes();
  const deleteRun = useDeleteRun();
  const writeRun = useWriteRun();
  const revertRun = useRevertRun();
  const isRunning = run.data?.outcome === "running";
  const hasSamples = run.data ? run.data.samples.length > 0 : false;
  const eligibility = run.data
    ? writeEligibility(run.data)
    : { eligible: false };
  const writes = run.data?.writes ?? [];
  const lastWrite = writes.length > 0 ? writes[writes.length - 1] : undefined;
  // Restore always targets "the last WriteKind::Write row" server-side (see
  // `require_writable_run`), so only offer it while that row is still the newest one — once
  // superseded by a later write or restore, showing Restore here would be misleading.
  const canRevertLastWrite =
    eligibility.eligible &&
    lastWrite !== undefined &&
    lastWrite.kind === "write" &&
    lastWrite.success;
  const stream = useRunStream(runId, isRunning);
  const initialReadings = stream.initialReadings ?? run.data?.initial_readings;
  // While running, the live SSE feed is the source of truth (it replays every sample from
  // tick 0); once terminal, fall back to `useRun`'s plain REST `samples`, which is cheaper
  // than keeping a stream open for a run that's already over.
  const trendSamples = isRunning
    ? stream.samples
    : (run.data?.samples ?? EMPTY_TREND_SAMPLES);
  const trendPollIntervalMs = run.data?.original_request?.poll_interval_ms;
  const trendPoints = useMemo(() => {
    if (!run.data) return [];

    return composeTrendPoints(
      trendSamples,
      initialReadings,
      run.data.started_at,
      run.data.completed_at,
      !isRunning && run.data.restore_status !== "incomplete",
    );
  }, [initialReadings, isRunning, run.data, trendSamples]);
  const [notes, setNotes] = useState("");
  const [notesDirty, setNotesDirty] = useState(false);

  useEffect(() => {
    if (run.data?.id === runId) {
      setNotes(run.data.notes ?? "");
      setNotesDirty(false);
    }
  }, [runId, run.data?.id, run.data?.notes]);

  function saveNotes() {
    updateNotes.mutate(
      { id: runId, notes },
      {
        onSuccess: (data) => {
          setNotes(data.notes ?? "");
          setNotesDirty(false);
        },
      },
    );
  }

  function clearNotes() {
    deleteNotes.mutate(runId, {
      onSuccess: (data) => {
        setNotes(data.notes ?? "");
        setNotesDirty(false);
      },
    });
  }

  return (
    <div>
      <PageHeading
        title={`Tune #${id ?? ""}`}
        actions={
          <>
            {isRunning && (
              <Button
                variant="danger"
                disabled={cancelRun.isPending}
                onClick={() => cancelRun.mutate(runId)}
              >
                {cancelRun.isPending ? "Cancelling…" : "Cancel tune"}
              </Button>
            )}
            {!isRunning && hasSamples && (
              <>
                <a href={runExportUrl(runId, "csv")} download>
                  <Button>Export CSV</Button>
                </a>
                <a href={runExportUrl(runId, "json")} download>
                  <Button>Export JSON</Button>
                </a>
              </>
            )}
            {!isRunning && (
              <Button
                variant="danger"
                disabled={deleteRun.isPending}
                onClick={() => {
                  if (
                    window.confirm(
                      `Delete tune #${runId}? This removes its recorded measurements and results and cannot be undone.`,
                    )
                  ) {
                    deleteRun.mutate(runId, {
                      onSuccess: () => navigate("/runs"),
                    });
                  }
                }}
              >
                {deleteRun.isPending ? "Deleting…" : "Delete tune"}
              </Button>
            )}
            <Button
              disabled={!run.data?.original_request}
              title={
                run.isSuccess && !run.data.original_request
                  ? "This tune's original settings weren't recorded and can't be duplicated."
                  : undefined
              }
              onClick={() => {
                if (!run.data?.original_request) return;
                const duplicateState: DuplicateRunState = {
                  duplicateRequest: run.data.original_request,
                  duplicateFromRunId: run.data.id,
                };
                navigate("/runs/new", { state: duplicateState });
              }}
            >
              Duplicate this run
            </Button>
            <Link to="/runs">
              <Button>Back to tune history</Button>
            </Link>
          </>
        }
      />

      {!Number.isFinite(runId) && (
        <ErrorBanner message={`"${id}" is not a valid run id.`} />
      )}
      {run.isPending && Number.isFinite(runId) && (
        <LoadingState message="Loading run…" />
      )}
      {run.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            run.error,
            "Unable to load tune details.",
          )}
        />
      )}
      {cancelRun.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            cancelRun.error,
            "Unable to cancel the tune.",
          )}
        />
      )}
      {deleteRun.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            deleteRun.error,
            "Unable to delete the tune.",
          )}
        />
      )}
      {updateNotes.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            updateNotes.error,
            "Unable to save notes.",
          )}
        />
      )}
      {deleteNotes.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            deleteNotes.error,
            "Unable to clear notes.",
          )}
        />
      )}
      {writeRun.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            writeRun.error,
            "Unable to apply PID settings.",
          )}
        />
      )}
      {revertRun.isError && (
        <ErrorBanner
          message={userFacingErrorMessage(
            revertRun.error,
            "Unable to restore the previous PID settings.",
          )}
        />
      )}

      {run.isSuccess && (
        <>
          {isRunning && (
            <div className="mb-6 rounded-lg border border-slate-700 bg-slate-900/60 px-4 py-3 text-sm">
              <p className="text-slate-300">
                Tune in progress — collecting live measurements.
                {stream.reconnecting && (
                  <span className="ml-2 text-amber-400">
                    Connection interrupted — retrying…
                  </span>
                )}
              </p>
              {stream.samples.length > 0 ? (
                (() => {
                  const latest = stream.samples[stream.samples.length - 1];
                  return (
                    <p className="mt-2 font-mono text-slate-400">
                      Tick {latest.tick_index}: PV {num(latest.sample.pv)}, MV{" "}
                      {num(latest.state.mv_value_current)}, cycles{" "}
                      {latest.state.cycles_completed} completed /{" "}
                      {latest.state.cycles_remaining} remaining
                    </p>
                  );
                })()
              ) : (
                <p className="mt-2 text-slate-500">
                  {stream.initialReadings
                    ? "Initial readings captured; waiting for the first measurement."
                    : "No measurements recorded yet."}
                </p>
              )}
            </div>
          )}

          <section className="mb-6">
            <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
              Trend
            </h2>
            {trendPoints.length === 0 ? (
              <p className="text-sm text-slate-500">
                No measurements recorded yet.
              </p>
            ) : (
              <TrendChart
                points={trendPoints}
                pollIntervalMs={trendPollIntervalMs}
              />
            )}
          </section>

          <Section title="Summary">
            <Field label="Tag name" value={run.data.tag_name} />
            <Field
              label="Outcome"
              value={
                <Badge tone={outcomeTone[run.data.outcome]}>
                  {OUTCOME_LABELS[run.data.outcome]}
                </Badge>
              }
            />
            <Field label="Driver" value={DRIVER_LABELS[run.data.driver]} />
            <Field
              label="Template"
              value={
                <>
                  {run.data.template_name}{" "}
                  <Badge tone={originTone[run.data.template_origin]}>
                    {run.data.template_origin}
                  </Badge>
                </>
              }
            />
            <Field label="Started" value={dateTime(run.data.started_at)} />
            <Field label="Completed" value={dateTime(run.data.completed_at)} />
            {run.data.failure_reason && (
              <Field
                label="Failure reason"
                value={run.data.failure_reason}
                full
              />
            )}
            {run.data.restore_status && (
              <Field
                label="Restore status"
                value={
                  <Badge tone={restoreTone[run.data.restore_status]}>
                    {run.data.restore_status}
                  </Badge>
                }
              />
            )}
            {run.data.restore_detail && (
              <Field
                label="Restore detail"
                value={run.data.restore_detail}
                full
              />
            )}
          </Section>

          <section className="mb-6">
            <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
              Notes
            </h2>
            <div className="rounded-lg border border-slate-800 bg-slate-900/40 p-5">
              <TextAreaField
                label="Run notes"
                value={notes}
                onChange={(value) => {
                  setNotes(value);
                  setNotesDirty(true);
                }}
                full
                placeholder="Optional context, observations, or follow-up actions"
                hint="Notes can be changed while the tune is active or after it finishes."
              />
              <div className="mt-3 flex gap-2">
                <Button
                  variant="primary"
                  disabled={!notesDirty || updateNotes.isPending}
                  onClick={saveNotes}
                >
                  {updateNotes.isPending ? "Saving…" : "Save notes"}
                </Button>
                <Button
                  variant="danger"
                  disabled={
                    deleteNotes.isPending ||
                    (!notesDirty && notes.trim().length === 0)
                  }
                  onClick={clearNotes}
                >
                  {deleteNotes.isPending ? "Clearing…" : "Clear notes"}
                </Button>
              </div>
            </div>
          </section>

          <Section title="Test configuration">
            <Field
              label="Process type"
              value={PROCESS_TYPE_LABELS[run.data.config.process_type]}
            />
            <Field
              label="Controller type"
              value={CONTROLLER_TYPE_LABELS[run.data.config.controller_type]}
            />
            <Field
              label="Relay amplitude"
              value={`${num(run.data.config.relay_amp_percent)}%`}
            />
            <Field
              label="Cycles (skip / count)"
              value={`${run.data.config.num_cycles_skip} / ${run.data.config.num_cycles_count}`}
            />
            <Field
              label="Noise protection"
              value={`${run.data.config.noise_protection_secs}s`}
            />
            <Field
              label="MRFT delay padding"
              value={`${run.data.config.mrft_delay_secs}s`}
            />
          </Section>

          {initialReadings && (
            <Section title="Initial readings">
              <Field label="PV initial" value={num(initialReadings.pv_ini)} />
              <Field label="MV initial" value={num(initialReadings.mv_ini)} />
              <Field
                label="PV range"
                value={`${num(initialReadings.pv_range_low)} – ${num(initialReadings.pv_range_high)}`}
              />
              <Field
                label="MV range"
                value={`${num(initialReadings.mv_range_low)} – ${num(initialReadings.mv_range_high)}`}
              />
              <Field
                label="Controller direction"
                value={DIRECTION_LABELS[initialReadings.controller_direction]}
              />
              <Field
                label="Setpoint initial"
                value={num(initialReadings.setpoint_ini)}
              />
              <Field
                label="Mode (raw)"
                value={initialReadings.mode_raw ?? "—"}
              />
              <Field
                label="Mode attribute (raw)"
                value={initialReadings.mode_attribute_raw ?? "—"}
              />
            </Section>
          )}

          <section className="mb-6">
            <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
              Calculated results
            </h2>
            {run.data.results.length === 0 ? (
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
                    {run.data.results.map((result) => (
                      <tr key={result.response_level}>
                        <td className="px-4 py-3 font-medium">
                          {RESPONSE_LEVEL_LABELS[result.response_level]}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.kp)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.ti_minutes)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.td_minutes)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.proportional)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.integral)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(result.derivative)}
                        </td>
                        <td className="px-4 py-3">
                          <Button
                            variant="primary"
                            disabled={
                              !eligibility.eligible || writeRun.isPending
                            }
                            title={eligibility.reason}
                            onClick={() => {
                              const tags = run.data.pid_constant_tags;
                              if (!eligibility.eligible || !tags) return;
                              const confirmed = window.confirm(
                                `Apply ${RESPONSE_LEVEL_LABELS[result.response_level]} PID constants to tag "${run.data.tag_name}"?\n\n` +
                                  `${tags.proportional}: ${num(result.proportional)}\n` +
                                  `${tags.integral}: ${num(result.integral)}\n` +
                                  `${tags.derivative}: ${num(result.derivative)}`,
                              );
                              if (confirmed) {
                                writeRun.mutate({
                                  id: runId,
                                  responseLevel: result.response_level,
                                });
                              }
                            }}
                          >
                            {writeRun.isPending &&
                            writeRun.variables?.responseLevel ===
                              result.response_level
                              ? "Writing…"
                              : "Apply"}
                          </Button>
                        </td>
                      </tr>
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

          <section className="mb-6">
            <h2 className="mb-3 text-sm font-semibold uppercase tracking-wide text-slate-400">
              PID change history
            </h2>
            {run.data.writes.length === 0 ? (
              <p className="text-sm text-slate-500">
                No PID settings were applied during this tune.
              </p>
            ) : (
              <div className="overflow-x-auto rounded-lg border border-slate-800">
                <table className="w-full text-left text-sm">
                  <thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
                    <tr>
                      <th className="px-4 py-2 font-medium">Action</th>
                      <th className="px-4 py-2 font-medium">Level</th>
                      <th className="px-4 py-2 font-medium">Changed at</th>
                      <th className="px-4 py-2 font-medium">Previous values</th>
                      <th className="px-4 py-2 font-medium">Applied values</th>
                      <th className="px-4 py-2 font-medium">
                        Read-back values
                      </th>
                      <th className="px-4 py-2 font-medium">Success</th>
                      <th className="px-4 py-2 font-medium">Rollback</th>
                      <th className="px-4 py-2 font-medium">Actions</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-800">
                    {run.data.writes.map((write, i) => (
                      <tr key={i}>
                        <td className="px-4 py-3">
                          {write.kind === "write" ? "Apply" : "Restore"}
                        </td>
                        <td className="px-4 py-3">
                          {RESPONSE_LEVEL_LABELS[write.response_level]}
                        </td>
                        <td className="px-4 py-3 text-slate-400">
                          {dateTime(write.written_at)}
                        </td>
                        <td className="px-4 py-3 font-mono text-slate-400">
                          {num(write.proportional_previous)} /{" "}
                          {num(write.integral_previous)} /{" "}
                          {num(write.derivative_previous)}
                        </td>
                        <td className="px-4 py-3 font-mono">
                          {num(write.proportional_written)} /{" "}
                          {num(write.integral_written)} /{" "}
                          {num(write.derivative_written)}
                        </td>
                        <td className="px-4 py-3 font-mono text-slate-400">
                          {num(write.proportional_readback)} /{" "}
                          {num(write.integral_readback)} /{" "}
                          {num(write.derivative_readback)}
                        </td>
                        <td className="px-4 py-3">
                          <Badge tone={write.success ? "success" : "error"}>
                            {write.success ? "Successful" : "Failed"}
                          </Badge>
                        </td>
                        <td className="px-4 py-3">
                          {write.rollback_state ? (
                            <Badge
                              tone={
                                write.rollback_state === "succeeded"
                                  ? "success"
                                  : "error"
                              }
                            >
                              {write.rollback_state === "succeeded"
                                ? "Restored"
                                : write.rollback_state === "failed"
                                  ? "Could not restore"
                                  : "Not needed"}
                            </Badge>
                          ) : (
                            <span className="text-slate-500">—</span>
                          )}
                        </td>
                        <td className="px-4 py-3">
                          {i === run.data.writes.length - 1 &&
                          canRevertLastWrite ? (
                            <Button
                              variant="danger"
                              disabled={revertRun.isPending}
                              title={eligibility.reason}
                              onClick={() => {
                                const tags = run.data.pid_constant_tags;
                                if (!eligibility.eligible || !tags) return;
                                const confirmed = window.confirm(
                                  `Restore the previous PID values on tag "${run.data.tag_name}"?\n\n` +
                                    `${tags.proportional}: ${num(write.proportional_previous)}\n` +
                                    `${tags.integral}: ${num(write.integral_previous)}\n` +
                                    `${tags.derivative}: ${num(write.derivative_previous)}`,
                                );
                                if (confirmed) {
                                  revertRun.mutate(runId);
                                }
                              }}
                            >
                              {revertRun.isPending
                                ? "Restoring…"
                                : "Restore previous values"}
                            </Button>
                          ) : (
                            <span className="text-slate-500">—</span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            {run.data.writes.some((w) => w.error_message) && (
              <div className="mt-3 space-y-2">
                {run.data.writes
                  .filter((w) => w.error_message)
                  .map((w, i) => (
                    <ErrorBanner
                      key={i}
                      message={`${RESPONSE_LEVEL_LABELS[w.response_level]}: ${
                        w.kind === "revert"
                          ? "The previous PID values could not be restored. Check the OPC connection and try again."
                          : "The PID settings could not be applied. Check the OPC connection and try again."
                      }`}
                    />
                  ))}
              </div>
            )}
          </section>

          <p className="text-sm text-slate-500">
            {trendSamples.length} measurements{" "}
            {isRunning ? "recorded so far" : "were recorded"} for this tune.
          </p>
        </>
      )}
    </div>
  );
}
